// Bold phrases on a page that tells a person what to find on screen must be
// what the console shows: a label from its message catalogue (a template's
// `{…}` stands for any text), or one of the page's own words (`own`).
import { catalogue } from "../../console/src/i18n/messages.ts";

const values = Object.values(catalogue as Record<string, string>);
const patterns = values.map(
  (v) => new RegExp(`^${v.replace(/[.*+?^$()|[\]\\]/g, "\\$&").replace(/\{\w+\}/g, ".+")}$`),
);

/** Whether `text` is something the console shows. */
export function isLabel(text: string): boolean {
  return values.includes(text) || patterns.some((p) => p.test(text));
}

/** The bold phrases of `markdown` (outside code) that are neither a console label nor in `own`. */
export function boldLabels(markdown: string, own: readonly string[]): string[] {
  const prose = markdown.replace(/^```[\s\S]*?^```/gm, "").replace(/`[^`\n]*`/g, "");
  const unknown: string[] = [];
  for (const m of prose.matchAll(/\*\*([^*]+)\*\*/g)) {
    const text = (m[1] ?? "").trim();
    if (own.includes(text) || isLabel(text)) continue;
    unknown.push(text);
  }
  return unknown;
}
