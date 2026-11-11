// What the server's refusals mean for each form, and what a role may offer
// on the members screen: pure functions, so the mapping is tested once and
// every screen shows a refusal on the field it concerns.
import type { ApiError } from "../api/client.ts";
import type { Permission, Role } from "../api/permissions.ts";
import { msg } from "../i18n/messages.ts";

/** A refusal on one field of a form, or `null` for one the form shows as a
 * whole (with `describe`). */
export type FieldRefusal<F extends string> = { field: F; message: string } | null;

/** The field an `invalid_field` refusal names, as the server spells it. */
const namedField = (e: ApiError): string | null => {
  const field = e.details["field"];
  return e.reason === "invalid_field" && typeof field === "string" ? field : null;
};

/** The limits the server holds an event's settings to (08 §6.2). */
export const EVENT_LIMITS = {
  holdTtl: { min: 60, max: 7_200, default: 900 },
  maxObjects: { min: 1, max: 100, default: 10 },
  tokensPerSub: { min: 1, max: 10, default: 2 },
} as const;

export type EventField = "key" | "name" | "chart" | "version" | "holdTtl" | "maxObjects" | "tokens";

export function eventRefusal(e: ApiError): FieldRefusal<EventField> {
  if (e.code === "already_exists") return { field: "key", message: msg("events.keyTaken") };
  if (e.code === "not_found" && e.details["missing"] === "chart") {
    return { field: "chart", message: msg("events.chartGone") };
  }
  if (e.code === "not_found" && e.details["missing"] === "version") {
    return { field: "version", message: msg("events.versionGone") };
  }
  if (e.reason === "archived") return { field: "chart", message: msg("events.chartArchived") };
  if (e.reason === "unpublished") {
    return { field: "chart", message: msg("events.chartUnpublished") };
  }
  const { holdTtl, maxObjects, tokensPerSub } = EVENT_LIMITS;
  switch (namedField(e)) {
    case "key":
      return { field: "key", message: msg("charts.textRule") };
    case "name":
      return { field: "name", message: msg("charts.textRule") };
    case "settings.hold_ttl_seconds":
      return { field: "holdTtl", message: msg("events.between", holdTtl) };
    case "settings.max_objects":
      return { field: "maxObjects", message: msg("events.between", maxObjects) };
    case "settings.max_tokens_per_sub":
      return { field: "tokens", message: msg("events.between", tokensPerSub) };
    default:
      return null;
  }
}

/** A whole number from `min` to `max` in `text`, or `null`. */
export function wholeNumber(text: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(text.trim())) return null;
  const n = Number(text.trim());
  return n >= min && n <= max ? n : null;
}

export type KeyField = "name" | "origins";

export function keyRefusal(e: ApiError): FieldRefusal<KeyField> {
  switch (namedField(e)) {
    case "name":
      return { field: "name", message: msg("charts.textRule") };
    case "allowed_origins":
      return { field: "origins", message: msg("keys.originsRule") };
    default:
      return null;
  }
}

/** The origins typed one per line (or separated by commas or spaces):
 * `null` when there are none, which allows every origin. */
export function parseOrigins(text: string): string[] | null {
  const list = text
    .split(/[\s,]+/)
    .map((o) => o.trim())
    .filter((o) => o !== "");
  return list.length === 0 ? null : list;
}

/** The origins the console can tell are not origins, before sending; the
 * server normalises and checks the rest. */
export function badOrigins(list: string[]): string[] {
  return list.filter((o) => {
    let url: URL;
    try {
      url = new URL(o);
    } catch {
      return true;
    }
    const bare = url.pathname === "/" && url.search === "" && url.hash === "" && !o.endsWith("/");
    return (url.protocol !== "https:" && url.protocol !== "http:") || !bare || url.username !== "";
  });
}

export type InviteField = "email" | "role";

const ownershipRefusal = (e: ApiError) =>
  e.code === "forbidden" && e.reason === "role" && e.details["permission"] === "ownership";

export function inviteRefusal(e: ApiError): FieldRefusal<InviteField> {
  if (e.code === "already_exists") return { field: "email", message: msg("invite.alreadyMember") };
  if (ownershipRefusal(e)) return { field: "role", message: msg("members.ownerOnly") };
  switch (namedField(e)) {
    case "email":
      return { field: "email", message: msg("invite.emailInvalid") };
    case "role":
      return { field: "role", message: msg("invite.roleInvalid") };
    default:
      return null;
  }
}

/** What a refused role change or removal says, or `null` for the general
 * description. `gone`: the person is no longer a member (reload the list). */
export function memberRefusal(e: ApiError): { message: string; gone: boolean } | null {
  if (e.reason === "last_owner") return { message: msg("members.lastOwner"), gone: false };
  if (ownershipRefusal(e)) return { message: msg("members.ownerOnly"), gone: false };
  if (e.code === "not_found") return { message: msg("members.gone"), gone: true };
  return null;
}

export const ROLES: Role[] = ["owner", "admin", "editor", "operator", "viewer"];

/** Why a member's role cannot be changed or the member removed by someone
 * with `permissions`: only an owner makes or unmakes an owner, and a
 * workspace keeps one owner. `owners` is the number of owners when the
 * whole list is loaded, `null` when it is not known (the server decides). */
export function memberLock(
  permissions: readonly Permission[],
  role: Role,
  owners: number | null,
): "ownership" | "lastOwner" | null {
  if (role !== "owner") return null;
  if (!permissions.includes("ownership")) return "ownership";
  return owners === 1 ? "lastOwner" : null;
}

/** The roles someone with `permissions` may give a member whose role is
 * `role` (`role` itself included), or invite as. */
export function roleChoices(permissions: readonly Permission[], role?: Role): Role[] {
  if (permissions.includes("ownership")) return ROLES;
  if (role === "owner") return ["owner"];
  return ROLES.filter((r) => r !== "owner");
}

export type AcceptRefusal =
  /** Wrong, expired, used or revoked: the link is spent. */
  | { kind: "spent" }
  /** Signed in as another email. */
  | { kind: "otherEmail" }
  /** The email has an account: sign in, then open the link again. */
  | { kind: "signIn" }
  | { kind: "password" }
  | { kind: "wait"; seconds: number }
  | { kind: "busy" }
  | null;

export function acceptRefusal(e: ApiError): AcceptRefusal {
  if (e.status === 401 && e.reason === "invitation") return { kind: "spent" };
  if (e.status === 403 && e.reason === "invitation_email") return { kind: "otherEmail" };
  if (e.status === 409 && e.reason === "sign_in") return { kind: "signIn" };
  if (namedField(e) === "password") return { kind: "password" };
  if (e.status === 429) return { kind: "wait", seconds: e.retryAfter ?? 60 };
  if (e.reason === "password_hashing") return { kind: "busy" };
  return null;
}
