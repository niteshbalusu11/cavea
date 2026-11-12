// The server's refusals mapped to the field or message they concern, and
// what each role is offered on the members screen (CON-007, CON-010,
// CON-012).
import { expect, test } from "bun:test";
import { ApiError, type ErrorCode } from "../api/client.ts";
import { OPERATIONS } from "../api/generated.ts";
import type { Permission, Role } from "../api/permissions.ts";
import { msg } from "../i18n/messages.ts";
import {
  acceptRefusal,
  badOrigins,
  originsProblem,
  originsRefusal,
  eventRefusal,
  inviteRefusal,
  keyRefusal,
  memberLock,
  memberRefusal,
  parseOrigins,
  roleChoices,
  settingsRefusal,
} from "./refusals.ts";

function problem(
  status: number,
  code: ErrorCode,
  details: Record<string, unknown> = {},
  retryAfter: number | null = null,
): ApiError {
  const reason = typeof details["reason"] === "string" ? details["reason"] : null;
  return new ApiError(status, code, reason, details, "req_1", retryAfter, "server text");
}
const field = (name: string) =>
  problem(400, "invalid_request", { reason: "invalid_field", field: name });

const on = (e: ApiError) => eventRefusal(e)?.field ?? null;
const kind = (e: ApiError) => acceptRefusal(e)?.kind ?? null;
const at = (e: ApiError) => settingsRefusal(e)?.field ?? null;

test("an event's refusals land on their fields", () => {
  expect(on(problem(409, "already_exists"))).toBe("key");
  expect(on(field("key"))).toBe("key");
  expect(on(field("name"))).toBe("name");
  expect(on(field("settings.hold_ttl_seconds"))).toBe("holdTtl");
  expect(on(field("settings.max_objects"))).toBe("maxObjects");
  expect(on(field("settings.max_tokens_per_sub"))).toBe("tokens");
  expect(on(problem(404, "not_found", { missing: "chart" }))).toBe("chart");
  expect(on(problem(404, "not_found", { missing: "version" }))).toBe("version");
  expect(on(problem(400, "invalid_request", { reason: "archived" }))).toBe("chart");
  expect(on(problem(400, "invalid_request", { reason: "unpublished" }))).toBe("chart");
  // Not a field's: the form shows it as a whole.
  expect(on(problem(403, "forbidden", { reason: "role" }))).toBeNull();
  expect(on(field("settings.unknown"))).toBeNull();
  expect(on(problem(500, "internal_error"))).toBeNull();
  expect(eventRefusal(field("settings.hold_ttl_seconds"))?.message).toBe(
    "Enter a whole number from 60 to 7,200.",
  );
});

test("a refused settings change lands on its field; archived is the event's, not a chart's", () => {
  expect(at(field("name"))).toBe("name");
  expect(at(field("settings.hold_ttl_seconds"))).toBe("holdTtl");
  expect(at(field("settings.max_objects"))).toBe("maxObjects");
  expect(at(field("settings.max_tokens_per_sub"))).toBe("tokens");
  expect(at(problem(400, "invalid_request", { reason: "archived" }))).toBeNull();
  // A create's fields that a change cannot have.
  expect(at(field("key"))).toBeNull();
  expect(at(problem(409, "already_exists"))).toBeNull();
  expect(at(problem(403, "forbidden", { reason: "role" }))).toBeNull();
});

test("a key's and an invitation's refusals land on their fields", () => {
  expect(keyRefusal(field("name"))?.field).toBe("name");
  expect(keyRefusal(field("allowed_origins"))?.field).toBe("origins");
  expect(keyRefusal(problem(403, "forbidden", { reason: "role" }))).toBeNull();
  expect(inviteRefusal(problem(409, "already_exists"))?.field).toBe("email");
  expect(inviteRefusal(field("email"))?.field).toBe("email");
  const ownership = problem(403, "forbidden", { reason: "role", permission: "ownership" });
  expect(inviteRefusal(ownership)?.field).toBe("role");
  expect(
    inviteRefusal(problem(403, "forbidden", { reason: "role", permission: "settings" })),
  ).toBeNull();
});

test("a refused role change or removal says why", () => {
  expect(memberRefusal(problem(400, "invalid_request", { reason: "last_owner" }))?.gone).toBe(
    false,
  );
  const ownership = problem(403, "forbidden", { reason: "role", permission: "ownership" });
  expect(memberRefusal(ownership)?.message).toContain("Only an owner");
  expect(memberRefusal(problem(404, "not_found", { missing: "member" }))?.gone).toBe(true);
  expect(
    memberRefusal(problem(403, "forbidden", { reason: "role", permission: "settings" })),
  ).toBeNull();
});

test("an invitation's acceptance refusals", () => {
  expect(kind(problem(401, "unauthorized", { reason: "invitation" }))).toBe("spent");
  expect(kind(problem(403, "forbidden", { reason: "invitation_email" }))).toBe("otherEmail");
  expect(kind(problem(409, "already_exists", { reason: "sign_in" }))).toBe("signIn");
  expect(kind(field("password"))).toBe("password");
  expect(acceptRefusal(problem(429, "rate_limited", {}, 12))).toEqual({
    kind: "wait",
    seconds: 12,
  });
  expect(kind(problem(503, "unavailable", { reason: "password_hashing" }))).toBe("busy");
  // An ended session is not the invitation's fault.
  expect(kind(problem(401, "unauthorized", { reason: "console_session" }))).toBeNull();
});

/** A role's permissions as the document grants them (as `me` lists them). */
function permissionsOf(role: Role): Permission[] {
  const out = new Set<Permission>();
  for (const op of Object.values(OPERATIONS)) {
    if (op.permission !== null && (op.roles as readonly string[]).includes(role)) {
      out.add(op.permission);
    }
  }
  return [...out];
}

test("only an owner makes, invites or unmakes an owner; one owner stays", () => {
  // `ownership` is no route's permission: the members routes check it
  // themselves, and `me` lists it for an owner only (09 §2).
  const admin = permissionsOf("admin");
  const owner: Permission[] = [...permissionsOf("owner"), "ownership"];
  expect(admin).not.toContain("ownership");

  // An admin: never an owner's role, and never offered "owner".
  expect(memberLock(admin, "owner", 3)).toBe("ownership");
  expect(memberLock(admin, "editor", 1)).toBeNull();
  expect(roleChoices(admin, "editor")).toEqual(["admin", "editor", "operator", "viewer"]);
  expect(roleChoices(admin)).not.toContain("owner");

  // An owner: everything, but the last owner stays an owner.
  expect(roleChoices(owner, "viewer")).toContain("owner");
  expect(memberLock(owner, "owner", 2)).toBeNull();
  expect(memberLock(owner, "owner", 1)).toBe("lastOwner");
  // With the list not wholly loaded, the server decides.
  expect(memberLock(owner, "owner", null)).toBeNull();
});

test("origins are read one per line and checked for a bare origin", () => {
  expect(parseOrigins("  \n ")).toBeNull();
  expect(parseOrigins("https://a.example\nhttp://localhost:5173, https://b.example")).toEqual([
    "https://a.example",
    "http://localhost:5173",
    "https://b.example",
  ]);
  expect(
    badOrigins([
      "https://a.example",
      "http://localhost:5173",
      "https://a.example/",
      "https://a.example/path",
      "ftp://a.example",
      "a.example",
      "https://user@a.example",
    ]),
  ).toEqual([
    "https://a.example/",
    "https://a.example/path",
    "ftp://a.example",
    "a.example",
    "https://user@a.example",
  ]);
  // As the server's rule: case is the server's to fold, IPv6 and ports are
  // origins, an empty host, a non-ASCII host and over 255 characters are not.
  expect(
    badOrigins([
      "HTTPS://Tickets.Example.com",
      "http://[::1]:8080",
      "https://",
      "https://exämple.com",
      `https://${"a".repeat(248)}`,
    ]),
  ).toEqual(["https://", "https://exämple.com", `https://${"a".repeat(248)}`]);
});

/** `originsRefusal` of a `400 invalid_request` with `reason`. */
const refusal = (reason: string, details: Record<string, unknown>) =>
  originsRefusal(new ApiError(400, "invalid_request", reason, details, null, null, ""));

test("a key's origins are 1 to 50 good ones, or every origin by choice; refusals land where they belong", () => {
  expect(originsProblem(true, "")).toBeNull();
  expect(originsProblem(false, " \n ")).toBe(msg("keys.originsNone"));
  expect(originsProblem(false, "https://a.example\nhttps://b.example")).toBeNull();
  const many = Array.from({ length: 51 }, (_, i) => `https://s${i}.example`).join("\n");
  expect(originsProblem(false, many)).toBe(msg("keys.originsTooMany"));
  expect(originsProblem(false, "https://a.example/")).toBe(
    msg("keys.originsBad", { list: "https://a.example/" }),
  );
  expect(refusal("invalid_field", { field: "allowed_origins" })).toEqual({
    field: true,
    message: msg("keys.originsRule"),
  });
  expect(refusal("revoked", {})).toEqual({ field: false, message: msg("keys.originsRevoked") });
  expect(refusal("invalid_field", { field: "name" })).toBeNull();
});
