// A password reset link's token (09 §2, D-90): `/console/reset#token=<64
// hex>`. As the invitation's (inviteToken.ts): taken from the fragment once
// at start, before the router and before the setup page's reader, removed
// from the address bar, kept in memory only, dropped once used or refused.
import { BASE } from "./paths.ts";
import { pageToken } from "./setupToken.ts";

/** The reset page's paths: the link's, and the router's own. */
export const RESET_PATHS = BASE === "" ? ["/console/reset", "/reset"] : ["/reset"];

const reset = pageToken(RESET_PATHS);

export const takeResetToken = reset.take;
export const resetToken = reset.get;
export const dropResetToken = reset.drop;
