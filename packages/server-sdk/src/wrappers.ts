// Which method of the client reaches each operation of the generated table,
// or `null` for one reachable only through `call` (05 §15). Typed against the
// table, so an operation the server gains fails `bun run typecheck` until it
// is given a line here; `wrappers.test.ts` calls each method and checks that
// it sends its operation.
import type { OperationId } from "./generated.ts";

export const WRAPPERS = {
  metrics: null,
  uploadAsset: "assets.upload",
  listCharts: "charts.list",
  createChart: "charts.create",
  archiveChart: "charts.archive",
  getChart: "charts.get",
  getDraft: "charts.getDraft",
  saveDraft: "charts.saveDraft",
  publishChart: "charts.publish",
  validateDraft: "charts.validate",
  listVersions: "charts.versions",
  getChartLock: "charts.getLock",
  lockChart: "charts.lock",
  unlockChart: "charts.unlock",
  setVersionThumbnail: "charts.setThumbnail",
  // The manager's binary frames, for the console's live map (08 §8.4).
  eventState: null,
  listEvents: "events.list",
  createEvent: "events.create",
  archiveEvent: "events.archive",
  getEvent: "events.get",
  updateEvent: "events.update",
  book: "events.book",
  changeStatus: "events.changeStatus",
  hold: "events.hold",
  listObjects: "events.listObjects",
  release: "events.release",
  createHoldToken: "holdTokens.create",
  releaseHoldToken: "holdTokens.release",
  getHoldToken: "holdTokens.get",
  extendHoldToken: "holdTokens.extend",
  listKeys: "keys.list",
  createKey: "keys.create",
  revokeKey: "keys.revoke",
  updateKeyOrigins: "keys.updateOrigins",
  listWorkspaces: "workspaces.list",
  createWorkspace: "workspaces.create",
  inviteWorkspaceOwner: "workspaces.inviteOwner",
  mintWorkspaceKey: "workspaces.mintKey",
  createOwnerPasswordReset: "workspaces.ownerPasswordReset",
} as const satisfies Record<OperationId, string | null>;
