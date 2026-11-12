// Answers the screens rely on, checked before they become state: a proxy's
// page or a wrong shape becomes an `unexpected` error the screen shows with
// Retry, rather than data that breaks it while rendering.
import { ApiError } from "./client.ts";
import type {
  AuditList,
  Chart,
  ChartList,
  ConsoleLicence,
  ConsoleMe,
  ConsoleSecondFactor,
  ConsoleTotpSecret,
  ConsoleWorkspace,
  Event,
  EventList,
  EventObject,
  InvitationCreated,
  InvitationList,
  Key,
  KeyCreated,
  KeyList,
  MemberList,
  PasswordResetCreated,
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

const isNumber = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isTextOrNull = (v: unknown): boolean => v === null || isText(v);
const isCount = (v: unknown): boolean => Number.isSafeInteger(v) && (v as number) >= 0;

/** The answer as a `ConsoleMe`, or an `unexpected` error. */
export function checkMe(v: unknown): ConsoleMe {
  const ok =
    isRecord(v) &&
    isRecord(v["user"]) &&
    isText(v["user"]["id"]) &&
    isText(v["user"]["email"]) &&
    typeof v["user"]["second_factor"] === "boolean" &&
    Array.isArray(v["workspaces"]) &&
    v["workspaces"].every(isWorkspace) &&
    isText(v["csrf_token"]) &&
    v["csrf_token"] !== "" &&
    isRecord(v["session"]);
  if (!ok) throw unreadable();
  return { ...(v as unknown as ConsoleMe), licence: readLicence(v["licence"]) };
}

const LICENCE_STATES = new Set(["valid", "grace", "expired", "missing", "invalid"]);
const isStrings = (v: unknown): boolean => Array.isArray(v) && v.every(isText);

/** The licence's state as `me` and the upload give it (12 §7.6). One the
 * console cannot read shows nothing and disables nothing: the server
 * refuses what the licence closes either way. */
export function readLicence(v: unknown): ConsoleLicence {
  const details = isRecord(v) ? v["licence"] : null;
  const ok =
    isRecord(v) &&
    isText(v["state"]) &&
    LICENCE_STATES.has(v["state"]) &&
    typeof v["read_only"] === "boolean" &&
    typeof v["manage"] === "boolean" &&
    isTextOrNull(v["grace_until"]) &&
    isTextOrNull(v["reason"]) &&
    (v["source"] === null || v["source"] === "setting" || v["source"] === "upload") &&
    (details === null ||
      (isRecord(details) &&
        ["id", "customer", "plan", "issued_at", "expires_at", "key_id"].every((k) =>
          isText(details[k]),
        ) &&
        isStrings(details["features"]) &&
        isStrings(details["domains"]) &&
        isNumber(details["grace_days"]) &&
        (details["max_workspaces"] === null || isNumber(details["max_workspaces"]))));
  return ok
    ? (v as unknown as ConsoleLicence)
    : {
        state: "valid",
        read_only: false,
        manage: false,
        grace_until: null,
        reason: null,
        source: null,
        licence: null,
      };
}

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
    (version === null || isVersion(version)) &&
    isRecord(v["events"]) &&
    isCount(v["events"]["active"]) &&
    isCount(v["events"]["archived"])
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
    isText(v["user"]["email"]) &&
    typeof v["user"]["second_factor"] === "boolean"
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

const STATUSES = new Set(["free", "held", "booked", "blocked"]);
const isStatus = (v: unknown) =>
  (isText(v) && STATUSES.has(v)) || (Number.isInteger(v) && Number(v) >= 4 && Number(v) <= 15);

function isObject(v: unknown): boolean {
  return (
    isRecord(v) &&
    isText(v["label"]) &&
    isNumber(v["oid"]) &&
    isStatus(v["status"]) &&
    (v["expires_at"] === undefined || isText(v["expires_at"])) &&
    (v["order_ref"] === undefined || isText(v["order_ref"]))
  );
}

/** A page of objects (`{data, next}`). */
export const checkObjectPage = (v: unknown) =>
  checkList<{ data: EventObject[]; next: string | null }>(v, isObject);

/** Objects by label (`{data, unknown}`). */
export function checkObjectsByLabel(v: unknown): { data: EventObject[]; unknown: string[] } {
  const ok =
    isRecord(v) &&
    Array.isArray(v["data"]) &&
    v["data"].every(isObject) &&
    Array.isArray(v["unknown"]) &&
    v["unknown"].every(isText);
  if (!ok) throw unreadable();
  return v as unknown as { data: EventObject[]; unknown: string[] };
}

/** A seat command's answer (`{seq, objects}`). */
export const checkSeatReply = (v: unknown) =>
  checkOne<{ seq: number; objects: { label: string; oid: number }[] }>(
    v,
    (r) =>
      isRecord(r) &&
      isNumber(r["seq"]) &&
      Array.isArray(r["objects"]) &&
      r["objects"].every((o) => isRecord(o) && isText(o["label"])),
  );

const isFilled = (v: unknown): v is string => isText(v) && v !== "";

/** A sign-in's `202`: a code is needed (D-89). */
export const checkChallenge = (v: unknown) =>
  checkOne<ConsoleSecondFactor>(
    v,
    (c) => isRecord(c) && isFilled(c["challenge"]) && isText(c["expires_at"]),
  );

/** A second factor's secret, to show once. */
export const checkTotpSecret = (v: unknown) =>
  checkOne<ConsoleTotpSecret>(
    v,
    (t) =>
      isRecord(t) &&
      isText(t["secret"]) &&
      /^[A-Z2-7]+=*$/.test(t["secret"]) &&
      isText(t["uri"]) &&
      t["uri"].startsWith("otpauth://totp/") &&
      isFilled(t["algorithm"]) &&
      Number.isInteger(t["digits"]) &&
      Number.isInteger(t["period"]),
  );

/** The recovery codes, to show once. */
export const checkRecoveryCodes = (v: unknown): string[] =>
  checkOne<{ recovery_codes: string[] }>(
    v,
    (r) =>
      isRecord(r) &&
      Array.isArray(r["recovery_codes"]) &&
      r["recovery_codes"].length > 0 &&
      r["recovery_codes"].length <= 100 &&
      r["recovery_codes"].every(isFilled),
  ).recovery_codes;

/** A member's reset link, to show once. */
export const checkPasswordResetCreated = (v: unknown) =>
  checkOne<PasswordResetCreated>(
    v,
    (c) =>
      isRecord(c) &&
      isFilled(c["link"]) &&
      isRecord(c["reset"]) &&
      isText(c["reset"]["id"]) &&
      isText(c["reset"]["user"]) &&
      isText(c["reset"]["expires_at"]) &&
      typeof c["reset"]["clear_second_factor"] === "boolean",
  );
