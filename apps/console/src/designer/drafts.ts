// The local safety net (07 §9, DSN-036): unsaved changes to a server
// draft, kept in the browser's IndexedDB, one record per editor (a tab's
// open chart) under its person, workspace and chart, with the etag they
// were based on. An editor writes and removes only its own record, so two
// tabs of one chart never delete or overwrite each other's work. While an
// editor is open it holds a Web Lock named by its record's key: opening the
// chart offers only records whose lock nobody holds (their tab closed or
// crashed), and acts on one (copies it, removes it) only while holding its
// lock and only if it is still exactly what was offered, so a live
// editor's record, or one rewritten since it was read, is never removed. IndexedDB is
// used directly: one object store, four operations.
import type { LocalStore } from "./autosave.ts";

const DB = "cavea-designer";
const STORE = "drafts";
/** The record's shape; a record of another version is discarded. */
const VERSION = 1;

export interface Recovery {
  v: typeof VERSION;
  /** `user/workspace/chart/editor`: whose, which, and which editor wrote it. */
  key: string;
  /** The server draft's etag the changes were made on. */
  etag: string;
  /** The chart document JSON, as the session exported it. */
  draft: string;
  /** When it was written, Unix ms. */
  at: number;
}

/** What became of a recovery action on a record (`claim`). */
export type Claimed =
  /** Done, holding the record's lock, the record as offered. */
  | "done"
  /** The record was rewritten or removed since it was read: read again. */
  | "changed"
  /** Another tab holds its lock: left alone. */
  | "busy"
  /** No Web Locks here: not done, the record kept. */
  | "unlocked";

/** Every record of a chart, and the keys of those that could not be read. */
export interface Stored {
  records: Recovery[];
  corrupt: string[];
}

/** The prefix of a person's records for a chart: `usr_…/ws_…/key/`. A chart
 * key cannot hold `/` (lowercase letters, digits, hyphens). */
export const recoveryPrefix = (user: string, workspace: string, chart: string): string =>
  `${user}/${workspace}/${chart}/`;

/** A stored value as a record for `key`, or `null` for anything else (an
 * older build's, a damaged one, another key's). */
export function parseRecovery(raw: unknown, key: string): Recovery | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (
    r["v"] !== VERSION ||
    r["key"] !== key ||
    typeof r["etag"] !== "string" ||
    typeof r["draft"] !== "string" ||
    typeof r["at"] !== "number" ||
    !Number.isFinite(r["at"])
  ) {
    return null;
  }
  return { v: VERSION, key, etag: r["etag"], draft: r["draft"], at: r["at"] };
}

/** Web Locks (every current browser, in a secure context), or `null`. */
const lockManager = (): LockManager | null =>
  (globalThis.navigator as { locks?: LockManager } | undefined)?.locks ?? null;

let database: Promise<IDBDatabase> | null = null;

function open(): Promise<IDBDatabase> {
  database ??= new Promise<IDBDatabase>((resolve, reject) => {
    if (typeof indexedDB === "undefined") throw new Error("IndexedDB is not available");
    const request = indexedDB.open(DB, 1);
    request.addEventListener("upgradeneeded", () => request.result.createObjectStore(STORE));
    request.addEventListener("success", () => resolve(request.result));
    request.addEventListener("error", () =>
      reject(request.error ?? new Error("IndexedDB did not open")),
    );
    request.addEventListener("blocked", () => reject(new Error("IndexedDB is blocked")));
  }).catch((e: unknown) => {
    // Not kept: the next write tries again.
    database = null;
    throw e;
  });
  return database;
}

async function run<T>(
  mode: IDBTransactionMode,
  act: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  const db = await open();
  return new Promise<T>((resolve, reject) => {
    const tx = db.transaction(STORE, mode);
    const request = act(tx.objectStore(STORE));
    // Resolved once committed, so a write that resolves is on disk.
    tx.addEventListener("complete", () => resolve(request.result));
    tx.addEventListener("error", () => reject(tx.error ?? new Error("IndexedDB failed")));
    tx.addEventListener("abort", () => reject(tx.error ?? new Error("IndexedDB aborted")));
  });
}

/** One chart's records, under `prefix` (`recoveryPrefix`). */
export function draftStore(prefix: string) {
  return {
    /** A new editor's own record: written and removed by it alone, its
     * lock held until `release`. */
    editor(): LocalStore {
      const key = prefix + crypto.randomUUID();
      let release: (() => void) | null = null;
      const locks = lockManager();
      // Granted before the first write, so no record of a live editor is
      // ever without its lock.
      const granted =
        locks === null
          ? Promise.resolve()
          : new Promise<void>((ok) => {
              locks
                .request(key, () => {
                  ok();
                  return new Promise<void>((done) => (release = done));
                })
                .catch(() => ok());
            });
      return {
        async put(draft, etag) {
          await granted;
          const record: Recovery = { v: VERSION, key, etag, draft, at: Date.now() };
          await run("readwrite", (s) => s.put(record, key));
        },
        async remove() {
          await run("readwrite", (s) => s.delete(key));
        },
        release: () => release?.(),
      };
    },
    /** Every record of the chart no open editor holds, newest first. */
    async read(): Promise<Stored> {
      const range = IDBKeyRange.bound(prefix, `${prefix}\uffff`);
      // The locks first, the records after: an editor lets its lock go only
      // after its last write, so a record whose lock was free before it was
      // read is final. (One whose editor took its lock after the query is
      // caught by `claim`.) Without Web Locks nothing is known live.
      const locks = lockManager();
      let live: Set<string>;
      try {
        live = new Set(
          locks === null
            ? []
            : ((await locks.query()).held ?? []).flatMap((l) =>
                l.name === undefined ? [] : [l.name],
              ),
        );
      } catch {
        return { records: [], corrupt: [] };
      }
      // One cursor in one transaction: each key with its own value.
      const pairs: [string, unknown][] = [];
      try {
        await run("readonly", (s) => {
          const cursor = s.openCursor(range);
          cursor.addEventListener("success", () => {
            const at = cursor.result;
            if (at === null) return;
            pairs.push([String(at.key), at.value]);
            at.continue();
          });
          return cursor;
        });
      } catch {
        // No store here (a private window): nothing to recover.
        return { records: [], corrupt: [] };
      }
      // A live editor's record (its lock held, in this tab or another) is
      // its own, not to be offered.
      const out: Stored = { records: [], corrupt: [] };
      for (const [key, value] of pairs) {
        if (live.has(key)) continue;
        const record = parseRecovery(value, key);
        if (record === null) out.corrupt.push(key);
        else out.records.push(record);
      }
      // oxlint-disable-next-line unicorn/no-array-sort -- `out.records` is ours; `toSorted` is ES2023, past the lib (ES2022).
      out.records.sort((a, b) => b.at - a.at);
      return out;
    },
    /** Removes a record that could not be read, by its key, only while
     * holding its lock: never a live editor's. Without Web Locks a record is
     * kept (offered again later) rather than risk that. */
    async remove(key: string): Promise<void> {
      const locks = lockManager();
      if (locks === null) return;
      await locks.request(key, { ifAvailable: true }, async (lock) => {
        if (lock !== null) await run("readwrite", (s) => s.delete(key));
      });
    },
    /** Runs `act` on `record` holding its lock, once the record is read
     * again and found exactly as offered; `drop` removes it. */
    async claim(
      record: Recovery,
      act: (drop: () => Promise<void>) => Promise<void>,
    ): Promise<Claimed> {
      const locks = lockManager();
      if (locks === null) return "unlocked";
      const key = record.key;
      return locks.request(key, { ifAvailable: true }, async (lock): Promise<Claimed> => {
        if (lock === null) return "busy";
        const now = parseRecovery(await run("readonly", (s) => s.get(key)), key);
        if (now?.at !== record.at || now.draft !== record.draft || now.etag !== record.etag) {
          return "changed";
        }
        await act(async () => {
          await run("readwrite", (s) => s.delete(key));
        });
        return "done";
      });
    },
  };
}
