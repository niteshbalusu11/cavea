// What a workspace's role allows (09 §2, CON-004). The server is the
// authority and checks every request; the interface only hides or disables
// what would be refused, from `me`'s permissions against each operation's
// declared `x-cavea-permission`.
import { OPERATIONS, type ConsoleMe, type OperationId } from "./generated.ts";

export type MeWorkspace = ConsoleMe["workspaces"][number];
export type Permission = MeWorkspace["permissions"][number];
export type Role = MeWorkspace["role"];

/** Whether a member of `workspace` may call `op`. An operation without a
 * permission needs none (the console's own session routes). */
export function allows(
  workspace: Pick<MeWorkspace, "permissions"> | null | undefined,
  op: OperationId,
): boolean {
  const needed: Permission | null = OPERATIONS[op].permission;
  if (needed === null) return true;
  return workspace?.permissions.includes(needed) === true;
}
