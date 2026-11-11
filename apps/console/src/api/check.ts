// Answers the screens rely on, checked before they become state: a proxy's
// page or a wrong shape becomes an `unexpected` error the screen shows with
// Retry, rather than data that breaks it while rendering.
import { ApiError } from "./client.ts";
import type { ChartList, ConsoleMe } from "./generated.ts";

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isText = (v: unknown): v is string => typeof v === "string";
const unreadable = () =>
  new ApiError(200, "unexpected", null, {}, null, null, "The answer could not be read.");

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
  if (!ok) throw unreadable();
  return v as unknown as ConsoleMe;
}

function isChart(v: unknown): boolean {
  if (!isRecord(v)) return false;
  const version = v["published_version"];
  return (
    isText(v["key"]) &&
    isText(v["name"]) &&
    isText(v["draft_updated_at"]) &&
    (v["archived_at"] === null || isText(v["archived_at"])) &&
    (version === null ||
      (isRecord(version) &&
        typeof version["number"] === "number" &&
        isRecord(version["stats"]) &&
        typeof version["stats"]["seats"] === "number"))
  );
}

/** A page of charts as a `ChartList`, or an `unexpected` error. */
export function checkChartList(v: unknown): ChartList {
  const ok =
    isRecord(v) &&
    Array.isArray(v["data"]) &&
    v["data"].every(isChart) &&
    (v["next"] === null || (isText(v["next"]) && v["next"] !== ""));
  if (!ok) throw unreadable();
  return v as unknown as ChartList;
}
