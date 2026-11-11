// Answers the screens rely on, checked before they become state: a proxy's
// page or a wrong shape becomes an `unexpected` error the screen shows with
// Retry, rather than data that breaks it while rendering.
import { ApiError } from "./client.ts";
import type {
  AuditList,
  Chart,
  ChartList,
  ConsoleMe,
  ConsoleWorkspace,
  Event,
  EventList,
  InvitationCreated,
  InvitationList,
  Key,
  KeyCreated,
  KeyList,
  MemberList,
  VersionList,
  WorkspaceMember,
} from "./generated.ts";

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

const isNumber = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isTextOrNull = (v: unknown): boolean => v === null || isText(v);

function isVersion(v: unknown): boolean {
  return (
    isRecord(v) &&
    isNumber(v["number"]) &&
    isText(v["created_at"]) &&
    isRecord(v["stats"]) &&
    isNumber(v["stats"]["seats"])
  );
}

function isChart(v: unknown): boolean {
  if (!isRecord(v)) return false;
  const version = v["published_version"];
  return (
    isText(v["key"]) &&
    isText(v["name"]) &&
    isText(v["draft_updated_at"]) &&
    isTextOrNull(v["archived_at"]) &&
    (version === null || isVersion(version))
  );
}

/** A page of a list: rows each passing `row`, and a cursor or `null`. */
function checkList<L>(v: unknown, row: (r: unknown) => boolean): L {
  const ok =
    isRecord(v) &&
    Array.isArray(v["data"]) &&
    v["data"].every(row) &&
    (v["next"] === null || (isText(v["next"]) && v["next"] !== ""));
  if (!ok) throw unreadable();
  return v as unknown as L;
}

/** One answer passing `ok`, or an `unexpected` error. */
function checkOne<T>(v: unknown, ok: (v: unknown) => boolean): T {
  if (!ok(v)) throw unreadable();
  return v as T;
}

const ROLES = new Set(["owner", "admin", "editor", "operator", "viewer"]);
const isRole = (v: unknown) => isText(v) && ROLES.has(v);
const isKind = (v: unknown) => v === "secret" || v === "publishable";

function isEvent(v: unknown): boolean {
  if (!isRecord(v)) return false;
  const settings = v["settings"];
  const counts = v["counts"];
  return (
    isText(v["key"]) &&
    isTextOrNull(v["name"]) &&
    isText(v["chart"]) &&
    isVersion(v["version"]) &&
    isText(v["created_at"]) &&
    isTextOrNull(v["archived_at"]) &&
    isRecord(settings) &&
    isNumber(settings["hold_ttl_seconds"]) &&
    isNumber(settings["max_objects"]) &&
    isNumber(settings["max_tokens_per_sub"]) &&
    typeof settings["require_session"] === "boolean" &&
    (counts === undefined ||
      (isRecord(counts) &&
        isNumber(counts["free"]) &&
        isNumber(counts["held"]) &&
        isNumber(counts["booked"]) &&
        isNumber(counts["blocked"])))
  );
}

function isKey(v: unknown): boolean {
  if (!isRecord(v)) return false;
  const origins = v["allowed_origins"];
  return (
    isText(v["id"]) &&
    isKind(v["kind"]) &&
    isText(v["name"]) &&
    isText(v["prefix"]) &&
    (origins === null || (Array.isArray(origins) && origins.every(isText))) &&
    isText(v["created_at"]) &&
    isTextOrNull(v["last_used_at"]) &&
    isTextOrNull(v["revoked_at"])
  );
}

function isMember(v: unknown): boolean {
  return (
    isRecord(v) &&
    isRole(v["role"]) &&
    isRecord(v["user"]) &&
    isText(v["user"]["id"]) &&
    isText(v["user"]["email"])
  );
}

function isInvitation(v: unknown): boolean {
  return (
    isRecord(v) &&
    isText(v["id"]) &&
    isText(v["email"]) &&
    isRole(v["role"]) &&
    isText(v["created_at"]) &&
    isText(v["expires_at"])
  );
}

function isAuditEntry(v: unknown): boolean {
  return (
    isRecord(v) &&
    isText(v["id"]) &&
    isText(v["at"]) &&
    isText(v["actor"]) &&
    isText(v["action"]) &&
    isTextOrNull(v["target"]) &&
    (v["detail"] === null || isRecord(v["detail"])) &&
    isTextOrNull(v["request_id"])
  );
}

export const checkChartList = (v: unknown) => checkList<ChartList>(v, isChart);
export const checkChart = (v: unknown) => checkOne<Chart>(v, isChart);
export const checkVersionList = (v: unknown) => checkList<VersionList>(v, isVersion);
export const checkEventList = (v: unknown) => checkList<EventList>(v, isEvent);
export const checkEvent = (v: unknown) => checkOne<Event>(v, isEvent);
export const checkKeyList = (v: unknown) => checkList<KeyList>(v, isKey);
export const checkKey = (v: unknown) => checkOne<Key>(v, isKey);
export const checkKeyCreated = (v: unknown) =>
  checkOne<KeyCreated>(v, (k) => isKey(k) && isRecord(k) && isText(k["key"]) && k["key"] !== "");
export const checkMemberList = (v: unknown) => checkList<MemberList>(v, isMember);
export const checkMember = (v: unknown) => checkOne<WorkspaceMember>(v, isMember);
export const checkInvitationList = (v: unknown) => checkList<InvitationList>(v, isInvitation);
export const checkInvitationCreated = (v: unknown) =>
  checkOne<InvitationCreated>(
    v,
    (c) => isRecord(c) && isInvitation(c["invitation"]) && isText(c["link"]) && c["link"] !== "",
  );
export const checkAuditList = (v: unknown) => checkList<AuditList>(v, isAuditEntry);
export const checkWorkspace = (v: unknown) =>
  checkOne<ConsoleWorkspace>(
    v,
    (w) => isRecord(w) && isText(w["id"]) && isText(w["key"]) && isText(w["name"]),
  );
