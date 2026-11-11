// The interface's gating against the role table the server enforces: each
// operation's `x-cavea-roles` in the OpenAPI document (09 §2, D-54).
import { expect, test } from "bun:test";
import document from "../../../../crates/cavea-server/openapi.json";
import { OPERATIONS, type OperationId } from "./generated.ts";
import { allows, type Permission, type Role } from "./permissions.ts";

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
