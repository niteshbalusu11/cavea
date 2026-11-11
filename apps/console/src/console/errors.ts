// What the console says about a failed request: one sentence for the
// person, the request id for a report. The server's own `message` is shown
// as text only, and only where the console has nothing better to say.
import type { ApiError } from "../api/client.ts";
import { isAbort } from "../api/client.ts";
import { asApiError } from "../api/query.ts";
import { msg } from "../i18n/messages.ts";
import { loadSession } from "../session.ts";
import { notify } from "../ui/overlays.tsx";

export function describe(e: ApiError): string {
  switch (e.code) {
    case "network":
      return msg("error.network");
    case "timeout":
      return msg("error.timeout");
    case "unexpected":
      if (e.status === 0) return msg("error.bug");
      // A proxy's answer for a server behind it that is down or slow.
      if (e.status === 502 || e.status === 503 || e.status === 504) return msg("error.network");
      return msg("error.unexpected", { status: e.status });
    case "rate_limited":
      return e.retryAfter === null
        ? msg("error.rateLimited")
        : msg("error.retryIn", { seconds: e.retryAfter });
    case "overloaded":
    case "unavailable":
      return msg("error.unavailable");
    case "internal_error":
      return msg("error.internal");
    case "forbidden":
      if (e.reason === "role") return msg("error.role");
      if (e.reason === "workspace") return msg("error.workspace");
      if (e.reason === "csrf" || e.reason === "origin") return msg("error.csrf");
      break;
    case "unauthorized":
      if (e.sessionLost) return msg("error.session");
      break;
  }
  return e.message === "" ? msg("error.status", { status: e.status }) : e.message;
}

/** "Request req_…", for a person to quote in a report. */
export function reference(e: ApiError): string | undefined {
  return e.requestId === null ? undefined : msg("error.requestId", { id: e.requestId });
}

/**
 * Reports a failed action in a toast. A cancelled request says nothing; an
 * ended session says nothing here (the frame sends the person to sign in).
 * A refusal for a role or a workspace re-reads `me`: the role may have
 * changed since the page was loaded, and the interface should follow it.
 */
export function report(e: unknown): void {
  if (isAbort(e)) return;
  const error = asApiError(e);
  if (error.sessionLost) return;
  if (error.code === "forbidden" && (error.reason === "role" || error.reason === "workspace")) {
    void loadSession();
  }
  notify(describe(error), "error", reference(error));
}
