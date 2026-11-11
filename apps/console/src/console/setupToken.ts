// The first-run setup link's token (09 §2): `/console/setup#token=<64 hex>`.
// The fragment never reaches a server or a `Referer`; the console takes it
// once at start, keeps it in memory only and removes it from the address
// bar, so it is not left in the history or a screenshot.

const TOKEN = /^[0-9a-f]{64}$/;

/** The token in a URL fragment such as `#token=…`: `null` when there is
 * none, `"malformed"` when there is one that cannot be a setup token. */
export function readSetupToken(hash: string): string | null | "malformed" {
  const params = new URLSearchParams(hash.startsWith("#") ? hash.slice(1) : hash);
  const token = params.get("token");
  if (token === null) return null;
  return TOKEN.test(token) ? token : "malformed";
}

let taken: string | null | "malformed" = null;

/** Takes the token from the address bar, once, before the router starts. */
export function takeSetupToken(): void {
  const found = readSetupToken(window.location.hash);
  if (found === null) return;
  taken = found;
  const { pathname, search } = window.location;
  window.history.replaceState(window.history.state, "", pathname + search);
}

export function setupToken(): string | null | "malformed" {
  return taken;
}

/** Forgets the token once it has been used. */
export function dropSetupToken(): void {
  taken = null;
}
