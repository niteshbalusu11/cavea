// The save state machine against a fake clock, server and store: work is
// never lost and never overwritten silently, whatever the network, another
// tab and the person do, in any order (07 §9, DSN-035, DSN-036).
import { expect, test } from "bun:test";
import {
  Autosave,
  LOCAL_DELAY_MS,
  LOCK_MS,
  MAX_RETRY_MS,
  RENEW_MS,
  SAVE_DELAY_MS,
  type LockInfo,
  type LockReply,
  type LockState,
  type SaveReply,
  type SaveState,
} from "./autosave.ts";

class FakeClock {
  t = 0;
  #next = 1;
  timers = new Map<number, { at: number; run: () => void }>();
  now = () => this.t;
  set = (run: () => void, ms: number) => {
    const id = this.#next++;
    this.timers.set(id, { at: this.t + ms, run });
    return id;
  };
  clear = (id: unknown) => void this.timers.delete(id as number);
  /** Runs every timer due within `ms`, in time order. */
  async advance(ms: number): Promise<void> {
    const end = this.t + ms;
    for (;;) {
      await settle();
      let first: [number, { at: number; run: () => void }] | undefined;
      for (const entry of this.timers)
        if (first === undefined || entry[1].at < first[1].at) first = entry;
      if (first === undefined || first[1].at > end) break;
      this.timers.delete(first[0]);
      this.t = first[1].at;
      first[1].run();
    }
    this.t = end;
    await settle();
  }
  /** A laptop's sleep: no timer runs for `ms`, then those due run at once. */
  async sleep(ms: number): Promise<void> {
    this.t += ms;
    await this.advance(0);
  }
}

const settle = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

type Mode = "up" | "down" | "signedOut";

/** Who holds the lock: `me`, this browser (every tab of it); `there`, the
 * same person in another browser, a holder apart (D-97); `other`, another
 * person. */
type Holder = "me" | "there" | "other";

/** The server's draft and etag, another writer, and requests the test
 * answers one at a time; the editing lock by the server's clock (D-73),
 * held by person and editor (D-97). */
class FakeServer {
  constructor(readonly clock: FakeClock) {}
  lock: { holder: Holder; since: number; until: number } | null = null;
  locks: { resolve: (r: LockReply) => void }[] = [];
  maxLocks = 0;
  /** Saves that arrived while another held a live lock (refused). */
  refusedLocked = 0;
  draft = "doc-0";
  etag = '"1"';
  #etags = 1;
  issued = new Set(['"1"']);
  mode: Mode = "up";
  pending: { draft: string; etag: string; resolve: (r: SaveReply) => void }[] = [];
  maxPending = 0;
  /** Another tab's save the editor has not seen. */
  unseen = false;
  /** The person chose to overwrite since the last other write. */
  overwriteChosen = false;
  silentOverwrites = 0;

  send = (draft: string, etag: string): Promise<SaveReply> =>
    new Promise((resolve) => {
      this.pending.push({ draft, etag, resolve });
      this.maxPending = Math.max(this.maxPending, this.pending.length);
    });

  live(): { holder: Holder; since: number; until: number } | null {
    return this.lock !== null && this.lock.until > this.clock.t ? this.lock : null;
  }

  info(l: { holder: Holder; since: number }): LockInfo {
    // The person's other browser is the person: the same holder id.
    const holder = l.holder === "other" ? "usr_other" : "usr_me";
    return { holder, email: null, since: `${l.since}` };
  }

  take = (): Promise<LockReply> =>
    new Promise((resolve) => {
      this.locks.push({ resolve });
      this.maxLocks = Math.max(this.maxLocks, this.locks.length);
    });

  /** Answers the oldest lock request as the server would now. */
  answerLock(): void {
    const r = this.locks.shift();
    if (r === undefined) return;
    if (this.mode === "down") return r.resolve({ kind: "offline" });
    if (this.mode === "signedOut") return r.resolve({ kind: "signedOut" });
    const live = this.live();
    if (live !== null && live.holder !== "me") {
      return r.resolve({ kind: "locked", lock: this.info(live) });
    }
    const since = live?.holder === "me" ? live.since : this.clock.t;
    this.lock = { holder: "me", since, until: this.clock.t + LOCK_MS };
    r.resolve({ kind: "held", lock: this.info(this.lock) });
  }

  /** Another person, or this person in another browser, takes the lock
   * (over this one's, if live). */
  takesOver(who: "there" | "other"): void {
    this.lock = { holder: who, since: this.clock.t, until: this.clock.t + LOCK_MS };
  }

  /** They save, as their designer may: only while the lock is theirs. */
  saves(who: "there" | "other"): void {
    if (this.live()?.holder === who) this.otherTabSaves();
  }

  /** They close the chart: their own lock is released, never another
   * editor's, the same person's included (D-97). */
  leaves(who: "there" | "other"): void {
    if (this.lock?.holder === who) this.lock = null;
  }

  /** Answers the oldest request as the server would now. */
  answer(): void {
    const r = this.pending.shift();
    if (r === undefined) return;
    if (!this.issued.has(r.etag)) throw new Error(`sent an etag never issued: ${r.etag}`);
    if (this.mode === "down") return r.resolve({ kind: "offline", retryAfterMs: null });
    if (this.mode === "signedOut") return r.resolve({ kind: "signedOut" });
    // The lock before the etag (08 §6.2).
    const live = this.live();
    if (live !== null && live.holder !== "me") {
      this.refusedLocked++;
      return r.resolve({ kind: "locked", lock: this.info(live) });
    }
    if (r.etag !== this.etag) return r.resolve({ kind: "conflict", etag: this.etag });
    if (this.unseen && !this.overwriteChosen) this.silentOverwrites++;
    this.unseen = false;
    this.overwriteChosen = false;
    this.draft = r.draft;
    r.resolve({ kind: "saved", etag: this.newEtag() });
  }

  newEtag(): string {
    this.etag = `"${++this.#etags}"`;
    this.issued.add(this.etag);
    return this.etag;
  }

  otherTabSaves(): void {
    this.draft = `other-${this.#etags}`;
    this.newEtag();
    this.unseen = true;
  }
}

class FakeStore {
  record: { draft: string; etag: string } | null = null;
  failing = false;
  released = 0;
  release = () => void this.released++;
  put = async (draft: string, etag: string) => {
    await Promise.resolve();
    if (this.failing) throw new Error("quota");
    this.record = { draft, etag };
  };
  remove = async () => {
    await Promise.resolve();
    this.record = null;
  };
}

function rig(start: { unsaved?: { stored: boolean } } = {}, locking = false) {
  const clock = new FakeClock();
  const server = new FakeServer(clock);
  const page = { active: true };
  const locks: LockState[] = [];
  /** Saves sent after the machine said another holds the lock. */
  let sentAfterLost = 0;
  if (locking) server.lock = { holder: "me", since: 0, until: LOCK_MS };
  const store = new FakeStore();
  // Recovered: the record is the document the editor opens.
  if (start.unsaved?.stored === true) store.record = { draft: "doc-0", etag: server.etag };
  /** `gone`: the session is disposed; reading it throws, as the module does. */
  const doc = { revision: 0, exports: 0, gone: false };
  const states: SaveState[] = [];
  const save = new Autosave(
    {
      clock,
      revision: () => {
        if (doc.gone) throw new Error("disposed");
        return doc.revision;
      },
      exportDraft: () => {
        doc.exports++;
        return `doc-${doc.revision}`;
      },
      send: (draft, etag) => {
        if (save.lock.kind === "lost") sentAfterLost++;
        return server.send(draft, etag);
      },
      store,
      onState: (s) => states.push(s),
      ...(locking
        ? {
            lock: {
              take: server.take,
              draftEtag: () => Promise.resolve(server.mode === "up" ? server.etag : null),
              active: () => page.active,
              onLock: (l: LockState) => locks.push(l),
            },
          }
        : {}),
    },
    {
      etag: server.etag,
      revision: 0,
      ...start,
      ...(locking ? { lock: server.info({ holder: "me", since: 0 }) } : {}),
    },
  );
  const edit = () => {
    doc.revision++;
    save.changed();
  };
  const latest = () => `doc-${doc.revision}`;
  return {
    clock,
    server,
    store,
    doc,
    save,
    states,
    edit,
    latest,
    page,
    locks,
    sentAfterLost: () => sentAfterLost,
  };
}

/** A small seeded generator, so a failure names its seed. */
function rng(seed: number) {
  let x = seed >>> 0 || 1;
  return (n: number) => {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    return (x >>> 0) % n;
  };
}

test("random edits, outages, other tabs, the lock and choices: nothing lost, nothing overwritten silently", async () => {
  const starts = [{}, { unsaved: { stored: true } }, { unsaved: { stored: false } }];
  for (let seed = 1; seed <= 600; seed++) {
    // Opened from the server, recovered from the store, or a file imported;
    // half of them holding the editing lock, which another person may take.
    const locking = seed % 2 === 0;
    const r = rig(starts[seed % 3], locking);
    if (seed % 3 !== 0) r.server.draft = "theirs";
    const next = rng(seed);
    for (let step = 0; step < 150; step++) {
      switch (next(locking ? 14 : 10)) {
        case 0:
        case 1:
        case 2:
          r.edit();
          break;
        case 3:
          await r.clock.advance(next(3_000));
          break;
        case 4:
        case 5:
          r.server.answer();
          break;
        case 6:
          r.server.mode = (["up", "up", "down", "signedOut"] as const)[next(4)] ?? "up";
          break;
        case 7:
          if (next(4) === 0) r.server.otherTabSaves();
          else r.save.retry();
          break;
        case 8: {
          // The person answers a conflict: keep theirs and overwrite with
          // the etag the conflict gave (the server's now, or a later one,
          // which is a conflict again).
          const s = r.save.state;
          if (s.kind === "conflict" && s.etag !== null) {
            r.server.overwriteChosen = true;
            r.save.overwrite(s.etag);
          }
          break;
        }
        case 9:
          r.store.failing = next(8) === 0;
          if (!r.store.failing) void r.save.keep();
          break;
        case 10:
          r.server.answerLock();
          break;
        case 11: {
          // Another person, or this person in another browser: takes over,
          // saves while theirs, or leaves.
          const who = next(2) === 0 ? "other" : "there";
          [() => r.server.takesOver(who), () => r.server.saves(who), () => r.server.leaves(who)][
            next(3)
          ]?.();
          break;
        }
        case 12:
          // The page hidden or shown (an idle hidden tab stops renewing).
          r.page.active = next(2) === 0;
          if (r.page.active) r.save.wake();
          break;
        case 13:
          // A long sleep: no timer runs, then everything due at once.
          await r.clock.sleep((1 + next(3)) * LOCK_MS);
          break;
      }
      await settle();
      expect(r.server.maxPending).toBeLessThanOrEqual(1);
      expect(r.server.maxLocks).toBeLessThanOrEqual(1);
      expect(r.server.silentOverwrites).toBe(0);
      // Once another holds the lock, nothing more is sent.
      expect(r.sentAfterLost()).toBe(0);
      // Once the store's throttle has run: the latest edits are on the
      // server under the etag held, or in the store, or both.
      if (!r.store.failing) {
        await r.clock.advance(LOCAL_DELAY_MS);
        const onServer = r.server.draft === r.latest() && r.server.etag === r.save.etag;
        const stored = r.store.record?.draft === r.latest();
        if (!onServer && !stored && r.save.unsaved) {
          throw new Error(`seed ${seed} step ${step}: the latest edits are nowhere`);
        }
        expect(r.save.kept).toBe(!r.save.unsaved || stored);
      }
    }
  }
});

test("saves two seconds after the last change, once", async () => {
  const r = rig();
  r.edit();
  await r.clock.advance(1_500);
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS - 1);
  expect(r.server.pending).toHaveLength(0);
  await r.clock.advance(1);
  expect(r.server.pending.map((p) => p.draft)).toEqual(["doc-2"]);
  expect(r.save.state.kind).toBe("saving");
  r.server.answer();
  await settle();
  expect(r.save.state).toEqual({ kind: "saved", at: r.clock.t });
  expect(r.server.draft).toBe("doc-2");
  // Saved: the store's record goes; a record it had was this draft's.
  expect(r.store.record).toBeNull();
});

test("a change during a save is followed by exactly one more save", async () => {
  const r = rig();
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  r.edit();
  // Two saves were due while the first was in flight: none sent yet.
  expect(r.server.pending).toHaveLength(1);
  r.server.answer();
  await settle();
  expect(r.server.pending.map((p) => p.draft)).toEqual(["doc-3"]);
  r.server.answer();
  await r.clock.advance(SAVE_DELAY_MS * 3);
  expect(r.server.pending).toHaveLength(0);
  expect(r.save.state.kind).toBe("saved");
});

test("offline: kept locally, retried at 2, 4, 8 s … up to a minute, edits do not hurry it", async () => {
  const r = rig();
  r.server.mode = "down";
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  const waits: number[] = [];
  for (let i = 0; i < 7; i++) {
    r.server.answer();
    await settle();
    const s = r.save.state;
    if (s.kind !== "offline") throw new Error(s.kind);
    waits.push(s.retryAt - r.clock.t);
    expect(r.store.record?.draft).toBe(r.latest());
    r.edit();
    await r.clock.advance(s.retryAt - r.clock.t - 1);
    expect(r.server.pending).toHaveLength(0);
    await r.clock.advance(1);
    expect(r.server.pending).toHaveLength(1);
  }
  expect(waits).toEqual([2_000, 4_000, 8_000, 16_000, 32_000, MAX_RETRY_MS, MAX_RETRY_MS]);
  r.server.mode = "up";
  r.server.answer();
  await settle();
  expect(r.server.draft).toBe(r.latest());
  expect(r.save.state.kind).toBe("saved");
  expect(r.store.record).toBeNull();
});

test("a conflict stops saving, keeps the work locally, and overwrites only when asked", async () => {
  const r = rig();
  r.server.otherTabSaves();
  const theirs = r.server.draft;
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  await settle();
  expect(r.save.state).toEqual({ kind: "conflict", etag: r.server.etag });
  expect(r.server.draft).toBe(theirs);
  expect(r.store.record).toEqual({ draft: "doc-1", etag: '"1"' });
  // More edits: kept locally, not sent.
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS * 5);
  expect(r.server.pending).toHaveLength(0);
  expect(r.store.record?.draft).toBe("doc-2");
  // Keep mine.
  r.save.overwrite(r.server.etag);
  r.server.overwriteChosen = true;
  r.server.answer();
  await settle();
  expect(r.server.draft).toBe("doc-2");
  expect(r.save.state.kind).toBe("saved");
});

test("signed out mid-save: the work is in the store before anything else happens", async () => {
  const r = rig();
  r.server.mode = "signedOut";
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  await settle();
  expect(r.save.state).toEqual({ kind: "signedOut" });
  await r.save.dispose();
  expect(r.store.record).toEqual({ draft: "doc-1", etag: '"1"' });
  // Released once the record is written: another tab may now recover it.
  expect(r.store.released).toBe(1);
  // Disposed: nothing more is sent.
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS * 10);
  expect(r.server.pending).toHaveLength(0);
});

test("a recovered draft is saved without an edit; flush waits for the server", async () => {
  const r = rig({ unsaved: { stored: true } });
  expect(r.save.unsaved).toBe(true);
  const flushed = r.save.flush();
  expect(r.server.pending).toHaveLength(1);
  r.server.answer();
  expect(await flushed).toBe(true);
  expect(r.server.draft).toBe("doc-0");
  // Nothing to save: at once.
  expect(await r.save.flush()).toBe(true);
  // Offline: false, and the work is kept.
  r.server.mode = "down";
  r.edit();
  const failed = r.save.flush();
  r.server.answer();
  expect(await failed).toBe(false);
  await settle();
  expect(r.store.record?.draft).toBe("doc-1");
});

test("exports once per revision, however often it saves and stores", async () => {
  const r = rig();
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.mode = "down";
  r.server.answer();
  await r.clock.advance(MAX_RETRY_MS);
  r.server.answer();
  await r.clock.advance(MAX_RETRY_MS);
  expect(r.doc.exports).toBe(1);
});

test("abandoned mid-save: the next editor starts from the etag that save got", async () => {
  const r = rig();
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  const etag = r.save.abandon();
  r.server.answer();
  expect(await etag).toBe(r.server.etag);
  expect(r.store.released).toBe(1);
  // Its session goes with the next editor's install, which reports the
  // change: it is never read again, and nothing more is kept or sent.
  r.doc.gone = true;
  expect(() => r.save.changed()).not.toThrow();
  expect(() => void r.save.keep()).not.toThrow();
  await r.clock.advance(SAVE_DELAY_MS * 3);
  expect([r.server.pending.length, r.store.record]).toEqual([0, null]);
});

test("two editors of one chart in one browser: neither's save or choice removes the other's work", async () => {
  // One store, a record per editor, as `drafts.ts` keys them.
  const shared = new Map<string, string>();
  const clock = new FakeClock();
  const server = new FakeServer(clock);
  const editor = (id: string) => {
    const doc = { revision: 0 };
    const save = new Autosave(
      {
        clock,
        revision: () => doc.revision,
        exportDraft: () => `${id}-${doc.revision}`,
        send: server.send,
        store: {
          put: async (draft) => void shared.set(id, draft),
          remove: async () => void shared.delete(id),
        },
        onState: () => {},
      },
      { etag: server.etag, revision: 0 },
    );
    return { doc, save, edit: () => (doc.revision++, save.changed()) };
  };
  const a = editor("a");
  const b = editor("b");
  a.edit();
  b.edit();
  await clock.advance(LOCAL_DELAY_MS);
  expect([shared.get("a"), shared.get("b")]).toEqual(["a-1", "b-1"]);
  await clock.advance(SAVE_DELAY_MS);
  // A's save lands; B's is a conflict.
  server.answer();
  server.answer();
  await settle();
  expect(a.save.state.kind).toBe("saved");
  expect(b.save.state.kind).toBe("conflict");
  expect([shared.has("a"), shared.get("b")]).toEqual([false, "b-1"]);
  // A replaces its document (a file over it): still only its own record goes.
  await a.save.abandon();
  expect(shared.get("b")).toBe("b-1");
});

test("signed out with a store that fails: not kept, so the page must not leave", async () => {
  const r = rig();
  r.store.failing = true;
  r.server.mode = "signedOut";
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  await settle();
  await r.save.dispose();
  expect([r.save.unsaved, r.save.kept, r.save.localFailed]).toEqual([true, false, true]);
});

/** A rig holding the lock, its first renewal due in 20 s. */
const locked = () => rig({}, true);

test("the lock is renewed every 20 s while the page is in use, and rests while it is hidden and idle", async () => {
  const r = locked();
  await r.clock.advance(RENEW_MS - 1);
  expect(r.server.locks).toHaveLength(0);
  await r.clock.advance(1);
  expect(r.server.locks).toHaveLength(1);
  r.server.answerLock();
  await r.clock.advance(RENEW_MS);
  expect(r.server.locks).toHaveLength(1);
  r.server.answerLock();
  // Hidden with nothing to save: no more renewals, no timer left.
  r.page.active = false;
  await r.clock.advance(RENEW_MS);
  expect(r.server.locks).toHaveLength(0);
  expect(r.clock.timers.size).toBe(0);
  // Hidden with changes on their way: renewed until they are saved.
  r.edit();
  r.save.wake();
  expect(r.server.locks).toHaveLength(1);
  r.server.answerLock();
  // Shown again a minute later: renewed at once; nobody took it, so it is
  // taken again, and the draft did not change: editing carries on.
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  await r.clock.sleep(LOCK_MS * 2);
  r.page.active = true;
  r.save.wake();
  r.server.answerLock();
  await settle();
  expect(r.save.lock.kind).toBe("held");
  expect(r.save.state.kind).toBe("saved");
});

test("taken over: saving stops, the work stays in the store, and the person learns who", async () => {
  const r = locked();
  r.edit();
  await r.clock.advance(LOCAL_DELAY_MS);
  r.server.takesOver("other");
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  await settle();
  expect(r.save.lock).toEqual({
    kind: "lost",
    lock: { holder: "usr_other", email: null, since: `${LOCAL_DELAY_MS}` },
    takenOver: true,
  });
  expect(r.save.state).toEqual({ kind: "locked" });
  expect(r.store.record?.draft).toBe("doc-1");
  // Edits that slip in (a gesture ending) are kept, not sent; no renewal.
  r.edit();
  await r.clock.advance(RENEW_MS * 3);
  expect([r.server.pending.length, r.server.locks.length]).toEqual([0, 0]);
  expect(r.store.record?.draft).toBe("doc-2");
  expect(r.server.refusedLocked).toBe(1);
});

test("the person's other browser is a holder apart (D-97): its leaving never frees this one's lock, and its take-over stops this one", async () => {
  const r = locked();
  r.edit();
  // The other browser opened read-only and closes: this one's lock stays.
  r.server.leaves("there");
  expect(r.server.live()?.holder).toBe("me");
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  await settle();
  expect(r.save.state.kind).toBe("saved");
  // It takes over: this one is refused at its next save, its work kept.
  r.server.takesOver("there");
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  await settle();
  expect(r.save.lock).toMatchObject({ kind: "lost", lock: { holder: "usr_me" }, takenOver: true });
  expect(r.store.record?.draft).toBe(r.latest());
  // Even once the other browser has left, nothing more is sent from here.
  r.server.leaves("there");
  await r.clock.advance(RENEW_MS * 3);
  expect([r.server.pending.length, r.server.locks.length]).toEqual([0, 0]);
});

test("a renewal refused is a take-over too; a lock that lapsed while the tab slept is not", async () => {
  const r = locked();
  r.server.takesOver("other");
  await r.clock.advance(RENEW_MS);
  r.server.answerLock();
  await settle();
  expect(r.save.lock.kind === "lost" && r.save.lock.takenOver).toBe(true);

  const s = locked();
  // Asleep for two minutes: the lock lapsed; another took it meanwhile.
  await s.clock.sleep(LOCK_MS * 2);
  s.server.takesOver("other");
  s.server.answerLock();
  await settle();
  expect(s.save.lock.kind === "lost" && !s.save.lock.takenOver).toBe(true);
});

test("a lock that lapsed is taken again; a draft saved meanwhile is a conflict, never overwritten", async () => {
  const r = locked();
  r.edit();
  await r.clock.advance(LOCAL_DELAY_MS);
  r.server.mode = "down";
  // Offline past the lock's minute: another person took it, saved and left.
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  for (let i = 0; i < 8; i++) {
    await r.clock.advance(RENEW_MS);
    r.server.answerLock();
  }
  expect(r.save.lock.kind).toBe("unrenewed");
  r.server.takesOver("other");
  r.server.saves("other");
  r.server.leaves("other");
  r.server.mode = "up";
  r.save.retry();
  r.server.answerLock();
  await settle();
  // The save that retry sent finds their draft: a conflict. Either way,
  // theirs was not overwritten and mine is in the store.
  r.server.answer();
  await settle();
  expect(r.save.state.kind).toBe("conflict");
  expect(r.server.silentOverwrites).toBe(0);
  expect(r.store.record?.draft).toBe("doc-1");
});

test("a lapse with nothing to save checks the draft: unchanged carries on, changed is a conflict", async () => {
  for (const changed of [false, true]) {
    const r = locked();
    r.page.active = false;
    await r.clock.sleep(LOCK_MS * 3);
    if (changed) {
      r.server.takesOver("other");
      r.server.saves("other");
      r.server.leaves("other");
    }
    r.page.active = true;
    r.save.wake();
    r.server.answerLock();
    await settle();
    expect(r.save.lock.kind).toBe("held");
    expect(r.save.state.kind).toBe(changed ? "conflict" : "saved");
  }
});

test("a renewal without an answer: editing goes on, retried at 2, 4, 8, 16 s, then every 20 s", async () => {
  const r = locked();
  r.server.mode = "down";
  await r.clock.advance(RENEW_MS);
  const waits: number[] = [];
  for (let i = 0; i < 6; i++) {
    r.server.answerLock();
    await settle();
    const l = r.save.lock;
    if (l.kind !== "unrenewed") throw new Error(l.kind);
    waits.push(l.retryAt - r.clock.t);
    await r.clock.advance(l.retryAt - r.clock.t);
  }
  expect(waits).toEqual([2_000, 4_000, 8_000, 16_000, RENEW_MS, RENEW_MS]);
  r.edit();
  await r.clock.advance(SAVE_DELAY_MS);
  r.server.answer();
  await settle();
  expect(r.save.state.kind).toBe("offline");
  expect(r.store.record?.draft).toBe("doc-1");
});
