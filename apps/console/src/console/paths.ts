// Where the console lives and where it may send a person back to.

/** The console's path prefix without a trailing slash: "" or, once the
 * server embeds the build under it (CON-015), the `base` Vite was given. */
export const BASE = (import.meta.env?.BASE_URL ?? "/").replace(/\/+$/, "");

/** The router path of `location` (without `BASE`), with its query. */
export function here(location: { pathname: string; search: string }): string {
  const p = location.pathname;
  const path = BASE !== "" && p.startsWith(BASE) ? p.slice(BASE.length) || "/" : p;
  return path + location.search;
}

const AUTH_PAGES = ["/sign-in", "/setup", "/console/setup"];

/** Where sign-in returns to: a path of this console from the `next`
 * parameter, never another origin (`//host`, `/\host`) or a sign-in page. */
export function safeNext(next: string | string[] | undefined): string {
  const value = Array.isArray(next) ? next[0] : next;
  if (
    value === undefined ||
    !value.startsWith("/") ||
    value.startsWith("//") ||
    value.startsWith("/\\") ||
    AUTH_PAGES.some((p) => value === p || value.startsWith(`${p}?`) || value.startsWith(`${p}/`))
  ) {
    return "/charts";
  }
  return value;
}

/** The sign-in page's address that returns to `path`. */
export function signInFor(path: string): string {
  const next = safeNext(path);
  return next === "/charts" ? "/sign-in" : `/sign-in?next=${encodeURIComponent(next)}`;
}
