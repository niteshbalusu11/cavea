// An invitation link's token (09 §7): `/console/invite#token=<64 hex>`. As
// the setup token (setupToken.ts): taken from the fragment once at start,
// before the router, removed from the address bar, kept in memory only.
// It stays in memory across a detour to sign-in, so a person with an
// account can sign in and come back to accept; it is dropped once used or
// refused as spent.
import { BASE } from "./paths.ts";
import { readSetupToken } from "./setupToken.ts";

/** The invitation page's paths: the link's, and the router's own. */
export const INVITE_PATHS = BASE === "" ? ["/console/invite", "/invite"] : ["/invite"];

let taken: string | null | "malformed" = null;

/** Takes the token when the address is the invitation page's; before
 * `takeSetupToken`, which would otherwise take it as a setup token. */
export function takeInviteToken(): void {
  const { pathname, search, hash } = window.location;
  const path = BASE !== "" && pathname.startsWith(BASE) ? pathname.slice(BASE.length) : pathname;
  if (!INVITE_PATHS.includes(path.replace(/\/+$/, ""))) return;
  const found = readSetupToken(hash);
  if (found === null) return;
  taken = found;
  window.history.replaceState(window.history.state, "", pathname + search);
}

export function inviteToken(): string | null | "malformed" {
  return taken;
}

/** Forgets the token: accepted, or refused as spent. */
export function dropInviteToken(): void {
  taken = null;
}
