// A chart's draft on the server (08 §6.2) as the designer uses it: opened
// with its etag, saved with `If-Match`, published with it. Every request
// names the chart's workspace itself, so a workspace chosen elsewhere
// meanwhile never redirects a save (09 §10).
import { ApiError, call, callWithHeaders } from "../api/client.ts";
import type { Chart, ChartDocument, Finding, Version } from "../api/generated.ts";
import { asApiError } from "../api/query.ts";
import { describe } from "../console/errors.ts";
import type { LockInfo, LockReply, SaveReply } from "./autosave.ts";
import { infoOf, lockOf, lockReplyOf, saveReply } from "./replies.ts";

/** Where a server chart lives. */
export interface ChartRef {
  /** `ws_…` */
  workspace: string;
  key: string;
}

export interface OpenedChart {
  chart: Chart;
  /** The draft as chart document JSON. */
  draft: string;
  /** The draft's etag: sent back as it came. */
  etag: string;
}

/** The chart and its draft. The etag is the draft answer's own `ETag`, so
 * it names exactly the draft read. */
export async function openChart(ref: ChartRef, signal: AbortSignal): Promise<OpenedChart> {
  const options = { workspace: ref.workspace };
  const chart = await call("getChart", { path: { chart: ref.key } }, signal, options);
  const got = await callWithHeaders("getDraft", { path: { chart: ref.key } }, signal, options);
  const etag = got.headers.get("ETag");
  if (etag === null || typeof got.reply !== "object" || got.reply === null) {
    throw new ApiError(got.status, "unexpected", null, {}, null, null, "No draft etag.");
  }
  return { chart, draft: JSON.stringify(got.reply), etag };
}

/** The draft's etag now (to overwrite after a conflict that did not say). */
export async function currentEtag(ref: ChartRef, signal: AbortSignal): Promise<string> {
  const chart = await call("getChart", { path: { chart: ref.key } }, signal, {
    workspace: ref.workspace,
  });
  return chart.draft_etag;
}

/** A failed save as what the autosave does next (`replies.ts`). */
export const saveReplyOf = (e: unknown): SaveReply => saveReply(e, describe);

/** Takes or renews the editing lock (07 §9), or takes it over. Never
 * rejects; an archived chart is `refused`. */
export async function takeLock(ref: ChartRef, takeOver = false): Promise<LockReply> {
  try {
    const lock = await call(
      "lockChart",
      { path: { chart: ref.key }, body: takeOver ? { take_over: true } : {} },
      new AbortController().signal,
      { workspace: ref.workspace },
    );
    return { kind: "held", lock: infoOf(lock) };
  } catch (e) {
    return lockReplyOf(e);
  }
}

/** Releases the lock if it is the caller's (`204` either way), as the page
 * is left: the request outlives it. Never rejects. */
export function releaseLock(ref: ChartRef): void {
  call("unlockChart", { path: { chart: ref.key } }, new AbortController().signal, {
    workspace: ref.workspace,
    keepalive: true,
  }).catch(() => {});
}

/** Who holds the lock, for a role that only reads; `null` for nobody (or
 * when it cannot be read). */
export async function readLock(ref: ChartRef, signal: AbortSignal): Promise<LockInfo | null> {
  try {
    const got = await call("getChartLock", { path: { chart: ref.key } }, signal, {
      workspace: ref.workspace,
    });
    return got.lock === null ? null : infoOf(got.lock);
  } catch {
    return null;
  }
}

/** Saves `draft` (chart document JSON from the session) over the draft
 * whose etag is `etag`. Never rejects. */
export async function saveDraft(ref: ChartRef, draft: string, etag: string): Promise<SaveReply> {
  try {
    // The export is sent as it is: parsing and serialising it again would
    // cost the main thread twice for nothing.
    const body = new Blob([draft], { type: "application/json" }) as unknown as ChartDocument;
    const chart = await call(
      "saveDraft",
      { path: { chart: ref.key }, headers: { "If-Match": etag }, body },
      // Never cancelled from here: a save on its way is answered or fails.
      new AbortController().signal,
      { workspace: ref.workspace },
    );
    return { kind: "saved", etag: chart.draft_etag };
  } catch (e) {
    return saveReplyOf(e);
  }
}

export type Published =
  | { kind: "published"; version: Version; created: boolean }
  /** The compile found errors (`422 chart_invalid`). */
  | { kind: "invalid"; findings: Finding[] }
  | { kind: "conflict" }
  /** Another holds the editing lock. */
  | { kind: "locked"; lock: LockInfo }
  | { kind: "failed"; error: ApiError };

/** Publishes the draft whose etag is `etag` (08 §6.2): what is compiled is
 * what the person saw saved. */
export async function publish(
  ref: ChartRef,
  etag: string,
  signal: AbortSignal,
): Promise<Published> {
  try {
    const got = await callWithHeaders(
      "publishChart",
      { path: { chart: ref.key }, headers: { "If-Match": etag } },
      signal,
      { workspace: ref.workspace },
    );
    return { kind: "published", version: got.reply, created: got.status === 201 };
  } catch (e) {
    const error = asApiError(e);
    if (error.code === "version_conflict") return { kind: "conflict" };
    const lock = lockOf(error);
    if (lock !== null) return { kind: "locked", lock };
    const findings = error.details["findings"];
    if (error.code === "chart_invalid" && Array.isArray(findings)) {
      return { kind: "invalid", findings: findings.filter(isFinding) };
    }
    return { kind: "failed", error };
  }
}

function isFinding(v: unknown): v is Finding {
  if (typeof v !== "object" || v === null) return false;
  const f = v as Record<string, unknown>;
  return (
    typeof f["code"] === "string" &&
    typeof f["message"] === "string" &&
    (f["severity"] === "error" || f["severity"] === "warning")
  );
}
