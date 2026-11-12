// An invitation link's token (09 §7): `/console/invite#token=<64 hex>`. As
// the setup token (setupToken.ts): taken from the fragment once at start,
// before the router, removed from the address bar, kept in memory only.
// It stays in memory across a detour to sign-in, so a person with an
// account can sign in and come back to accept; it is dropped once used or
// refused as spent.
import { BASE } from "./paths.ts";
import { pageToken } from "./setupToken.ts";

/** The invitation page's paths: the link's, and the router's own. */
export const INVITE_PATHS = BASE === "" ? ["/console/invite", "/invite"] : ["/invite"];

const invite = pageToken(INVITE_PATHS);

/** Takes the token when the address is the invitation page's; before
 * `takeSetupToken`, which would otherwise take it as a setup token. */
export const takeInviteToken = invite.take;
export const inviteToken = invite.get;
/** Forgets the token: accepted, or refused as spent. */
export const dropInviteToken = invite.drop;
