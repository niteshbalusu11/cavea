// Theme tokens as CSS custom properties (docs/05-web-sdk.md §8). The built-in
// interface reads `--cavea-*` through the shadow boundary with `var()`, over
// the built-in scheme's defaults; the canvas gets the same values through
// `View.setTheme`, read from the computed style when they change.
import type { ThemeName } from "./options.ts";

/**
 * The built-in schemes' colours the interface uses: the engine's own
 * defaults (`Theme::builtin` in cavea-render), so controls and canvas agree.
 */
export const SCHEMES: Record<ThemeName, Record<string, string>> = {
  light: {
    bg: "#fafaf9",
    fg: "#1c1917",
    "fg-muted": "#57534e",
    "seat-unavailable-fill": "#e7e5e4",
    "seat-unavailable-stroke": "#78716c",
    "seat-selected-fill": "#1c1917",
    "seat-selected-ink": "#fafaf9",
    "ring-focus": "#2563eb",
  },
  dark: {
    bg: "#1c1917",
    fg: "#fafaf9",
    "fg-muted": "#a8a29e",
    "seat-unavailable-fill": "#292524",
    "seat-unavailable-stroke": "#a8a29e",
    "seat-selected-fill": "#fafaf9",
    "seat-selected-ink": "#1c1917",
    "ring-focus": "#60a5fa",
  },
  "high-contrast": {
    bg: "#000000",
    fg: "#ffffff",
    "fg-muted": "#ffffff",
    "seat-unavailable-fill": "#1f1f1f",
    "seat-unavailable-stroke": "#ffffff",
    "seat-selected-fill": "#ffff00",
    "seat-selected-ink": "#000000",
    "ring-focus": "#ffff00",
  },
};

/** The tokens the canvas reads from CSS, besides `category-<key>`. */
export const CANVAS_TOKENS = (
  "bg fg fg-muted seat-available-stroke seat-unavailable-fill seat-unavailable-stroke " +
  "seat-selected-fill seat-selected-ink ring-hover ring-focus section-tint dim font"
).split(" ");

/** Whether a token (without `--cavea-`) holds a colour. */
export const isColourToken = (name: string): boolean => !/^(section-tint|dim|font)$/.test(name);

/** Each scheme's private properties: the `--cavea-` token, else the default. */
export const schemeCss = (): string =>
  Object.entries(SCHEMES)
    .map(
      ([scheme, tokens]) =>
        `.r[data-s=${scheme}]{${Object.entries(tokens)
          .map(([name, value]) => `--_${name}:var(--cavea-${name},${value})`)
          .join(";")}}`,
    )
    .join("");
