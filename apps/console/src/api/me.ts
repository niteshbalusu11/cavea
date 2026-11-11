// `me` as the console relies on it: checked before it becomes the session,
// so a proxy's page or a wrong shape cannot replace a working session with
// one that breaks every screen.
import { ApiError } from "./client.ts";
import type { ConsoleMe } from "./generated.ts";

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isText = (v: unknown): v is string => typeof v === "string";

function isWorkspace(v: unknown): boolean {
  return (
    isRecord(v) &&
    isText(v["id"]) &&
    isText(v["key"]) &&
    isText(v["name"]) &&
    isText(v["role"]) &&
    Array.isArray(v["permissions"]) &&
    v["permissions"].every(isText)
  );
}

/** The answer as a `ConsoleMe`, or an `unexpected` error. */
export function checkMe(v: unknown): ConsoleMe {
  const ok =
    isRecord(v) &&
    isRecord(v["user"]) &&
    isText(v["user"]["id"]) &&
    isText(v["user"]["email"]) &&
    Array.isArray(v["workspaces"]) &&
    v["workspaces"].every(isWorkspace) &&
    isText(v["csrf_token"]) &&
    v["csrf_token"] !== "" &&
    isRecord(v["session"]);
  if (!ok) {
    throw new ApiError(200, "unexpected", null, {}, null, null, "The session could not be read.");
  }
  return v as unknown as ConsoleMe;
}
