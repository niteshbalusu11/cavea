// The interface's gating against the role table the server enforces: each
// operation's `x-cavea-roles` in the OpenAPI document (09 §2, D-54).
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import document from "../../../../crates/cavea-server/openapi.json";
import { OPERATIONS, type OperationId } from "./generated.ts";
import {
  allows,
  CLOSED_WHILE_EXPIRED,
  followLicence,
  refusal,
  type Permission,
  type Role,
} from "./permissions.ts";

const ROLES: Role[] = ["owner", "admin", "editor", "operator", "viewer"];
const ops = Object.entries(OPERATIONS) as [OperationId, (typeof OPERATIONS)[OperationId]][];

/** A role's permissions as the document implies them: every permission of
 * an operation the role may call. `me` lists the same for a member. */
function permissionsOf(role: Role): Permission[] {
  const out = new Set<Permission>();
  for (const [, op] of ops) {
    if (op.permission !== null && (op.roles as readonly string[]).includes(role)) {
      out.add(op.permission);
    }
  }
  return [...out];
}

test("the generated table matches the document", () => {
  const paths = document.paths as Record<string, Record<string, Record<string, unknown>>>;
  for (const [id, op] of ops) {
    const source = paths[op.path]?.[op.method.toLowerCase()];
    expect(source?.["operationId"]).toBe(id);
    expect(source?.["x-cavea-permission"] ?? null).toBe(op.permission);
  }
});

test("a role is offered exactly the operations its permissions allow", () => {
  // One permission gives one set of roles wherever it is declared, so
  // gating by `me.permissions` equals gating by each operation's roles.
  for (const role of ROLES) {
    const workspace = { permissions: permissionsOf(role) };
    for (const [id, op] of ops) {
      if (op.permission === null) continue;
      expect([id, role, allows(workspace, id)]).toEqual([
        id,
        role,
        (op.roles as readonly string[]).includes(role),
      ]);
    }
  }
});

const can = (role: Role, op: OperationId) => allows({ permissions: permissionsOf(role) }, op);

test("the cells of 09 §2 the console shows today", () => {
  expect(ROLES.filter((r) => can(r, "listCharts"))).toEqual(ROLES);
  expect(ROLES.filter((r) => can(r, "createChart"))).toEqual(["owner", "admin", "editor"]);
  expect(ROLES.filter((r) => can(r, "archiveChart"))).toEqual(["owner", "admin"]);
  expect(ROLES.filter((r) => can(r, "listKeys"))).toEqual(["owner", "admin"]);
  expect(ROLES.filter((r) => can(r, "listMembers"))).toEqual(["owner", "admin"]);
  // No workspace, nothing.
  expect(allows(null, "listCharts")).toBe(false);
  // The session's own routes need no permission.
  expect(allows(null, "consolePassword")).toBe(true);
});

const SERVER = join(import.meta.dir, "../../../../crates/cavea-server/src");

test("while the licence is expired, exactly the server's chart and event writes are closed", () => {
  // The server's rule, read from its source: `Permission::closed_while_expired`.
  const source = readFileSync(join(SERVER, "console/mod.rs"), "utf8");
  const rule = /fn closed_while_expired[\s\S]*?matches!\(\s*self,([^)]*)\)/.exec(source)?.[1] ?? "";
  const variants = [...rule.matchAll(/Self::(\w+)/g)].map((m) => m[1]);
  const names = variants.map(
    (v) => new RegExp(`Self::${v} => "([^"]+)"`).exec(source)?.[1] ?? `unknown ${v}`,
  );
  expect(new Set(names)).toEqual(new Set(CLOSED_WHILE_EXPIRED));
  // And the routes 12 §7.6 lists: nothing more is disabled, nothing less.
  const owner = { permissions: permissionsOf("owner") };
  const closed = ops
    .filter(([id]) => refusal(owner, id, true) === "licence")
    .map(([, op]) => `${op.method} ${op.path}`);
  expect(new Set(closed)).toEqual(
    new Set([
      "POST /v1/charts",
      "PUT /v1/charts/{chart}/draft",
      "POST /v1/charts/{chart}/validate",
      "POST /v1/charts/{chart}/publish",
      "PUT /v1/charts/{chart}/versions/{number}/thumbnail",
      "POST /v1/charts/{chart}/lock",
      "DELETE /v1/charts/{chart}/lock",
      "DELETE /v1/charts/{chart}",
      "POST /v1/assets",
      "POST /v1/events",
      "PATCH /v1/events/{event}",
      "DELETE /v1/events/{event}",
    ]),
  );
  for (const role of ROLES) {
    const workspace = { permissions: permissionsOf(role) };
    for (const [id] of ops) {
      const valid = refusal(workspace, id, false);
      const expired = refusal(workspace, id, true);
      // The role is the role whatever the licence; the licence only closes.
      expect(valid === "licence").toBe(false);
      expect(expired === "role").toBe(valid === "role");
    }
  }
  // Seat operations never close (D-20).
  const seats = ops.filter(([, op]) => op.permission?.startsWith("seats.") === true);
  expect(seats.length).toBeGreaterThan(5);
  for (const [id] of seats) {
    expect([id, refusal(owner, id, true)]).toEqual([id, null]);
  }
});

test("`allows` follows the licence the session reports", () => {
  const owner = { permissions: permissionsOf("owner") };
  let expired = false;
  followLicence(() => expired);
  try {
    expect(allows(owner, "createChart")).toBe(true);
    expired = true;
    expect([allows(owner, "createChart"), allows(owner, "createKey")]).toEqual([false, true]);
  } finally {
    followLicence(() => false);
  }
});
