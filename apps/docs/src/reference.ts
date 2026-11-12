// The parts of the written pages that come from the product's own source, so
// they cannot drift from it. A page asks for one with a line of its own:
//
//   <!-- members: SeatMapOptions options -->   an interface's members as a table
//                                              (`options`: all but callbacks,
//                                              `callbacks`: only those); object
//                                              types and `UiOptions` expand in place
//   <!-- members: react:SeatMapProps -->       the same, from @cavea/react's source
//   <!-- union: CaveaErrorCode -->             a string union, a row per member
//   <!-- declaration: Snapshot -->             a declaration as TypeScript
//   <!-- messages -->                          the renderer's English messages
//   <!-- theme-defaults -->                    the built-in schemes' colours
//   <!-- shortcuts -->                         the designer's keyboard shortcuts
//   <!-- metrics -->                           the server's Prometheus metrics
//
// The build fails when a member or union member printed here has no doc
// comment, when a name the renderer's entry module exports is printed by no
// page, when the theming page's token table differs from the tokens the
// renderer reads, or when the deployment page's settings table differs from
// the settings the server reads, or when a page names a metric (`cavea_…` in
// code) that the server does not have.

import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { GROUPS, helpSections } from "../../console/src/designer/shortcuts.ts";
import { msg } from "../../console/src/i18n/messages.ts";
import { MESSAGES } from "../../../packages/renderer/src/messages.ts";
import { CANVAS_TOKENS, SCHEMES } from "../../../packages/renderer/src/theme.ts";
import { customer } from "./api.ts";
import { Declarations, members, type Member } from "./declarations.ts";

export interface Sources {
  renderer: Declarations;
  react: Declarations;
  /** The server's metrics: name, type, help, from the Rust that writes them. */
  metrics: Array<{ name: string; kind: string; help: string }>;
  /** Names a page has printed, renderer's and React's (`react:` prefixed). */
  claimed: Set<string>;
  problems: string[];
}

export async function sources(root: string): Promise<Sources> {
  const api = await readFile(join(root, "packages/renderer/api.md"), "utf8");
  const react = await readFile(join(root, "packages/react/src/index.ts"), "utf8");
  const metrics: Sources["metrics"] = [];
  for (const file of ["crates/cavea-engine/src/metrics.rs", "crates/cavea-server/src/metrics.rs"]) {
    const rust = await readFile(join(root, file), "utf8");
    // `out.family(name, kind, help)`, `out.gauge(name, help, …)`, `out.counter(…)`, `out.histogram_family(name, help, …)`.
    for (const m of rust.matchAll(
      /\.(family|gauge|counter|histogram_family)\(\s*"(cavea_\w+)",\s*"([^"]*)"(?:,\s*"([^"]*)")?/g,
    )) {
      const [, call = "", name = "", second = "", third] = m;
      const kind = call === "family" ? second : call === "histogram_family" ? "histogram" : call;
      const help = call === "family" ? (third ?? "") : second;
      metrics.push({ name, kind, help });
    }
  }
  return {
    metrics,
    renderer: Declarations.fromApiMarkdown(api, "packages/renderer/api.md"),
    react: new Declarations(new Map([["index.ts", react]]), "packages/react/src/index.ts"),
    claimed: new Set(),
    problems: [],
  };
}

const cell = (text: string): string => text.replace(/\|/g, "\\|").replace(/\s*\n+\s*/g, " ");
const code = (text: string): string => `\`${cell(text)}\``;
/** A doc comment as a customer reads it: no reference to the internal specifications. */
const describe = (doc: string): string => cell(customer(doc.replace(/\n\n/g, " ")));
const isCallback = (m: Member): boolean => /^on[A-Z]/.test(m.name) && m.type.includes("=>");

/** Interfaces whose members are listed under the member that has them as its type. */
const EXPAND = new Set(["UiOptions"]);

function memberRows(
  s: Sources,
  d: Declarations,
  list: Member[],
  owner: string,
  prefix: string,
  filter: string | undefined,
  marks: boolean,
): string[] {
  const rows: string[] = [];
  for (const m of list) {
    if (filter === "options" && isCallback(m)) continue;
    if (filter === "callbacks" && !isCallback(m)) continue;
    const name = `${prefix}${m.name}`;
    const doc = describe(m.doc);
    // An object type without a description of its own is listed member by member.
    const literal = !m.method && doc === "" && m.raw.startsWith("{") && m.raw.endsWith("}");
    if (literal || EXPAND.has(m.type)) {
      if (doc !== "") rows.push(`| ${code(name)} | ${code(m.type)} | ${doc} |`);
      const inner = literal ? members(m.raw.slice(1, -1)) : d.interface(m.type);
      if (!literal) s.claimed.add(m.type);
      rows.push(...memberRows(s, d, inner, owner, `${name}.`, undefined, marks));
      continue;
    }
    if (doc === "") {
      s.problems.push(
        `${owner}.${name} has no description: add a doc comment to its declaration (${d.where})`,
      );
    }
    rows.push(`| ${code(`${name}${marks && m.optional ? "?" : ""}`)} | ${code(m.type)} | ${doc} |`);
  }
  return rows;
}

function membersTable(s: Sources, spec: string, filter: string | undefined): string {
  const react = spec.startsWith("react:");
  const name = react ? spec.slice("react:".length) : spec;
  const d = react ? s.react : s.renderer;
  s.claimed.add(spec);
  const list = d.interface(name);
  const marks = filter === undefined;
  const head =
    filter === "callbacks"
      ? "| Callback | Signature | Called when |"
      : list.every((m) => m.method)
        ? "| Method | Signature | What it does |"
        : "| Name | Type | Description |";
  const rows = memberRows(s, d, list, name, "", filter, marks);
  return [head, "|---|---|---|", ...rows].join("\n");
}

function unionTable(s: Sources, name: string): string {
  s.claimed.add(name);
  const rows = s.renderer.union(name).map((u) => {
    const doc = describe(u.doc);
    if (doc === "")
      s.problems.push(`${name} "${u.value}" has no description (${s.renderer.where})`);
    return `| ${code(u.value)} | ${doc} |`;
  });
  return ["| Code | Meaning |", "|---|---|", ...rows].join("\n");
}

function declaration(s: Sources, name: string): string {
  s.claimed.add(name);
  const doc = s.renderer.doc(name);
  const text = customer(s.renderer.source(name))
    .split("\n")
    .map((l) => customer(l))
    .join("\n");
  const comment =
    doc === ""
      ? ""
      : `/**\n${customer(doc)
          .split("\n")
          .map((l) => ` * ${l}`.trimEnd())
          .join("\n")}\n */\n`;
  return `\`\`\`ts\n${comment}${text}\n\`\`\``;
}

function messages(s: Sources): string {
  for (const n of ["Message", "MessageId", "Messages"]) s.claimed.add(n);
  const rows = Object.entries(MESSAGES).map(([id, m]) => {
    const english =
      typeof m === "string"
        ? m
        : Object.entries(m as Record<string, string>)
            .map(([form, text]) => `${form}: ${text}`)
            .join(" · ");
    return `| ${code(id)} | ${cell(english)} |`;
  });
  return ["| Id | English |", "|---|---|", ...rows].join("\n");
}

function themeDefaults(): string {
  const schemes = Object.keys(SCHEMES) as Array<keyof typeof SCHEMES>;
  const names = Object.keys(SCHEMES.light);
  const rows = names.map(
    (n) =>
      `| ${code(`--cavea-${n}`)} | ${schemes.map((sc) => code(SCHEMES[sc][n] ?? "")).join(" | ")} |`,
  );
  return [
    `| Token | ${schemes.map((sc) => code(sc)).join(" | ")} |`,
    `|---|${schemes.map(() => "---|").join("")}`,
    ...rows,
  ].join("\n");
}

function shortcuts(): string {
  const pc = helpSections(false);
  const mac = helpSections(true);
  const out: string[] = [];
  for (const [i, section] of pc.entries()) {
    const rows = section.rows
      .map((row, j) => ({ row, macKeys: mac[i]?.rows[j]?.keys ?? [] }))
      .filter(({ row }) => row.pending === undefined);
    if (rows.length === 0) continue;
    const title = section.group === "tree" ? "Layers panel" : msg(`group.${section.group}`);
    out.push(
      `### ${title}`,
      "",
      `| ${msg("help.pc")} | ${msg("help.mac")} | Action |`,
      "|---|---|---|",
      ...rows.map(
        ({ row, macKeys }) =>
          `| ${row.keys.map(code).join(" or ")} | ${macKeys.map(code).join(" or ")} | ${cell(msg(row.label))} |`,
      ),
      "",
    );
  }
  void GROUPS;
  return out.join("\n").trim();
}

function metricsTable(s: Sources): string {
  if (s.metrics.length < 15) {
    s.problems.push(
      `found only ${s.metrics.length} metrics in the server's source: the scan in reference.ts needs updating`,
    );
  }
  const rows = s.metrics.map((m) => {
    if (m.help === "")
      s.problems.push(`the metric ${m.name} has no help text in the server's source`);
    return `| ${code(m.name)} | ${m.kind} | ${cell(m.help)} |`;
  });
  return ["| Metric | Type | What it measures |", "|---|---|---|", ...rows].join("\n");
}

/** Metric names a page quotes (`cavea_…_total`, with or without labels) that the server does not have. */
export function unknownMetrics(s: Sources, page: string, source: string): string[] {
  const known = new Set(s.metrics.map((m) => m.name));
  const out: string[] = [];
  for (const m of source.matchAll(/`(cavea_[a-z_]+)(?:\{[^`]*\})?`/g)) {
    const name = m[1] ?? "";
    const metric = /_(total|seconds|size|depth|loaded|active|subscribers|connections|in_use)$/;
    if (metric.test(name) && !known.has(name)) {
      out.push(`${page}: names the metric ${name}, which the server does not have`);
    }
  }
  return out;
}

const DIRECTIVE =
  /^<!-- (members|union|declaration|messages|theme-defaults|shortcuts|metrics)(?::\s*([\w:]+)(?:\s+(\w+))?)?\s*-->$/gm;

/** `source` with every directive line replaced by what it asks for. */
export function expand(source: string, page: string, s: Sources): string {
  const out = source.replace(DIRECTIVE, (_, kind: string, name?: string, filter?: string) => {
    try {
      switch (kind) {
        case "members":
          if (name === undefined) throw new Error("members needs a name");
          return membersTable(s, name, filter);
        case "union":
          if (name === undefined) throw new Error("union needs a name");
          return unionTable(s, name);
        case "declaration":
          if (name === undefined) throw new Error("declaration needs a name");
          return declaration(s, name);
        case "messages":
          return messages(s);
        case "theme-defaults":
          return themeDefaults();
        case "metrics":
          return metricsTable(s);
        default:
          return shortcuts();
      }
    } catch (e) {
      s.problems.push(`${page}: ${(e as Error).message}`);
      return "";
    }
  });
  for (const m of out.matchAll(/§|docs\/\d\d-|\(D-\d+\)/g)) {
    const at = out.slice(Math.max(0, (m.index ?? 0) - 50), (m.index ?? 0) + 30).replace(/\n/g, " ");
    s.problems.push(
      `${page}: an internal specification reference reached the page from a declaration: …${at}…`,
    );
  }
  const left = out.match(/^<!--\s*(?:members|union|declaration)\b.*-->$/m);
  if (left) s.problems.push(`${page}: a directive the build does not understand: ${left[0]}`);
  return out;
}

/** After every page: each name the renderer exports was printed somewhere. */
export function unclaimed(s: Sources): string[] {
  return s.renderer
    .exported()
    .filter((n) => !s.claimed.has(n))
    .map((n) => `@cavea/renderer exports ${n}, which no page prints (add a directive for it)`);
}

/** Rows of a written table whose first cell is one code span matching `pattern`. */
function tableNames(markdown: string, pattern: RegExp): Set<string> {
  const out = new Set<string>();
  for (const line of markdown.split("\n")) {
    const m = line.match(/^\|\s*`([^`]+)`\s*\|/);
    if (m && pattern.test(m[1] ?? "")) out.add(m[1] ?? "");
  }
  return out;
}

function differ(
  what: string,
  documented: Set<string>,
  actual: Set<string>,
  where: string,
): string[] {
  const problems: string[] = [];
  for (const a of actual)
    if (!documented.has(a)) problems.push(`${a} is not in ${what} (${where})`);
  for (const d of documented)
    if (!actual.has(d)) problems.push(`${what} names ${d}, which ${where} does not have`);
  return problems;
}

/** The theming page's token table against the tokens the renderer reads. */
export async function themeTokens(root: string, page: string): Promise<string[]> {
  const ui = await readFile(join(root, "packages/renderer/src/ui.ts"), "utf8");
  const actual = new Set(CANVAS_TOKENS.map((t) => `--cavea-${t}`));
  for (const m of ui.matchAll(/--cavea-([a-z]+(?:-[a-z]+)*)(-?)/g)) {
    actual.add(m[2] === "-" ? `--cavea-${m[1]}-<key>` : `--cavea-${m[1]}`);
  }
  return differ(
    "the theming page's token table",
    tableNames(page, /^--cavea-/),
    actual,
    "the renderer (theme.ts, ui.ts)",
  );
}

/** The settings page's table against the variables the server's configuration reads. */
export async function settings(root: string, page: string): Promise<string[]> {
  const config = await readFile(join(root, "crates/cavea-server/src/config.rs"), "utf8");
  const actual = new Set(
    [...config.matchAll(/(?:setting|optional|var_os)\("(CAVEA_[A-Z0-9_]+)"/g)].map(
      (m) => m[1] ?? "",
    ),
  );
  if (actual.size < 10)
    return [`found only ${actual.size} settings in config.rs: the scan needs updating`];
  return differ(
    "the settings table",
    tableNames(page, /^CAVEA_/),
    actual,
    "crates/cavea-server/src/config.rs",
  );
}
