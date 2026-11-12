// What a workspace's role allows (09 §2, CON-004), and what the licence
// allows now (12 §7.6). The server is the authority and checks every
// request; the interface only hides or disables what would be refused, from
// `me`'s permissions against each operation's declared `x-cavea-permission`,
// and from `me.licence.read_only`. `refusal` is the one rule: every screen's
// `allows` goes through it, so none can forget the licence.
import { OPERATIONS, type ConsoleMe, type OperationId } from "./generated.ts";

export type MeWorkspace = ConsoleMe["workspaces"][number];
export type Permission = MeWorkspace["permissions"][number];
export type Role = MeWorkspace["role"];

/** What a console session is refused while the licence is expired: writing
 * charts and events, and nothing else (the server's
 * `Permission::closed_while_expired`; seat operations never, D-20). */
export const CLOSED_WHILE_EXPIRED: readonly Permission[] = [
  "charts.write",
  "charts.archive",
  "events.write",
];

/** Why a member of `workspace` may not call `op` now: the role, or the
 * expired licence (`readOnly`); `null` when they may. An operation without
 * a permission needs none (the console's own session routes). */
export function refusal(
  workspace: Pick<MeWorkspace, "permissions"> | null | undefined,
  op: OperationId,
  readOnly: boolean,
): "role" | "licence" | null {
  const needed: Permission | null = OPERATIONS[op].permission;
  if (needed === null) return null;
  if (workspace?.permissions.includes(needed) !== true) return "role";
  return readOnly && CLOSED_WHILE_EXPIRED.includes(needed) ? "licence" : null;
}

/** Where the licence's read-only state is read: set by `session.ts`. */
let licenceReadOnly = (): boolean => false;
export function followLicence(readOnly: () => boolean): void {
  licenceReadOnly = readOnly;
}

/** `refusal` for the signed-in person now. */
export const refused = (
  workspace: Pick<MeWorkspace, "permissions"> | null | undefined,
  op: OperationId,
): "role" | "licence" | null => refusal(workspace, op, licenceReadOnly());

/** Whether a member of `workspace` may call `op` now. */
export const allows = (
  workspace: Pick<MeWorkspace, "permissions"> | null | undefined,
  op: OperationId,
): boolean => refused(workspace, op) === null;
