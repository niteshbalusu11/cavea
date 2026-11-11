// Light and dark themes (09 §8): the system's by default, or the one the
// user picked, applied as `data-theme` on the root element.
import { createSignal } from "solid-js";
import { load, save } from "./storage.ts";

export type ThemeChoice = "system" | "light" | "dark";
const KEY = "cavea.theme.v1";
const CHOICES: readonly ThemeChoice[] = ["system", "light", "dark"];

const [choice, setChoiceSignal] = createSignal<ThemeChoice>(
  load(KEY, (v) => CHOICES.find((c) => c === v), "system"),
);
export { choice as themeChoice };

const media = window.matchMedia("(prefers-color-scheme: dark)");

const [resolved, setResolved] = createSignal<"light" | "dark">("light");
/** The theme applied: the choice, or the system's. */
export { resolved as resolvedTheme };

function apply(): void {
  const c = choice();
  const dark = c === "dark" || (c === "system" && media.matches);
  document.documentElement.dataset["theme"] = dark ? "dark" : "light";
  setResolved(dark ? "dark" : "light");
}

export function setThemeChoice(c: ThemeChoice): void {
  setChoiceSignal(c);
  save(KEY, c);
  apply();
}

/** Applies the theme now and whenever the system's changes. */
export function followTheme(): void {
  media.addEventListener("change", apply);
  apply();
}
