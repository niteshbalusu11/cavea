// The colour tokens meet WCAG 2.2 AA in both themes (11 §8): text 4.5:1
// on every surface it is drawn on, controls and focus 3:1 (1.4.11).
import { expect, test } from "bun:test";

const css = await Bun.file(new URL("./styles.css", import.meta.url)).text();

function block(selector: string): Record<string, string> {
  const at = css.indexOf(`${selector} {`);
  const body = css.slice(at, css.indexOf("}", at));
  return Object.fromEntries(
    [...body.matchAll(/--c-([\w-]+):\s*(#[0-9a-f]{6})/g)].map((m) => [m[1] ?? "", m[2] ?? ""]),
  );
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
}

function ratio(a: string, b: string): number {
  const [la, lb] = [luminance(a), luminance(b)];
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

const TEXT = ["fg", "muted", "faint", "accent", "danger", "warning", "success"];
const SURFACES = ["app", "surface", "raised", "hover", "active", "control", "accent-soft"];

for (const [theme, selector] of [
  ["light", ":root"],
  ["dark", ':root[data-theme="dark"]'],
] as const) {
  test(`the ${theme} theme's text and controls meet AA`, () => {
    const t = block(selector);
    const low: string[] = [];
    const need = (fg: string, bg: string, min: number) => {
      const r = ratio(t[fg] ?? "#000000", t[bg] ?? "#000000");
      if (r < min) low.push(`${fg} on ${bg}: ${r.toFixed(2)}`);
    };
    for (const fg of TEXT)
      for (const bg of SURFACES) if (!(fg === "accent" && bg === "accent-soft")) need(fg, bg, 4.5);
    need("fg", "danger-soft", 4.5);
    need("danger", "danger-soft", 4.5);
    need("fg", "warning-soft", 4.5);
    need("warning", "warning-soft", 4.5);
    need("on-accent", "accent-fill", 4.5);
    for (const bg of ["app", "surface", "raised", "control"]) {
      need("control-line", bg, 3);
      need("focus", bg, 3);
    }
    expect(low).toEqual([]);
  });
}
