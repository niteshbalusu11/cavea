// Saving a server draft (07 §9, DSN-035 and DSN-036): two seconds after
// the last change the draft is sent with `If-Match`; unsaved changes are
// also kept in the browser's store, at most once a second. The machine has
// no DOM: the clock, the network and the store are given to it, so its
// behaviour is tested without a browser (`autosave.test.ts`).
//
// What it guarantees: the person's latest edits are on the server under the
// etag the machine holds, or in the local store, or both, once the store's
// throttle has run; a record is removed from the store only after the server
// holds that revision or a later one. It never sends two saves at once (a
// change during a save is followed by one more), and never overwrites a
// draft it has not seen: the etag is the server's, sent back as it came, and
// a conflict stops saving until the person chooses.
//
// The editing lock (07 §9, DSN-037, D-73) is coordination, not safety: the
// machine renews it every 20 seconds while the page is in use, and stops
// saving when another holds it (a renewal or a save answered
// `chart_locked`), keeping the work in the store as for a conflict. The
// etag stays the guard: a lock that lapsed and was taken again has the
// draft's etag checked, and a draft changed meanwhile is a conflict.

/** How long after the last change the draft is sent (07 §9). */
export const SAVE_DELAY_MS = 2_000;
/** The local store is written at most this often. */
export const LOCAL_DELAY_MS = 1_000;
/** Retries after a failure wait 2 s, 4 s, … up to this. */
export const MAX_RETRY_MS = 60_000;
/** The lock is renewed this often (08 §6.2: it lasts 60 s unless renewed). */
export const RENEW_MS = 20_000;
/** How long the server keeps a lock nobody renews. */
export const LOCK_MS = 60_000;

/** Who holds a chart's editing lock (the server's `ChartLock`). */
export interface LockInfo {
  /** `usr_…`, or `key_…` for an API key. */
  holder: string;
  /** For those whose role may edit charts; otherwise `null`. */
  email: string | null;
  /** RFC 3339: when the holder took it. A renewal keeps it. */
  since: string;
}

/** The answer to taking or renewing the lock. */
export type LockReply =
  | { kind: "held"; lock: LockInfo }
  /** Another holds it (`409 chart_locked`). */
  | { kind: "locked"; lock: LockInfo }
  | { kind: "offline" }
  | { kind: "signedOut" }
  /** Refused for a reason retrying will not change (an archived chart). */
  | { kind: "refused" };

export type LockState =
  /** No lock: a machine started without one, or the page let it go. */
  | { kind: "none" }
  | { kind: "held"; lock: LockInfo }
  /** A renewal got no answer: editing goes on, kept locally if the saves
   * fail too; renewed again at `retryAt`. */
  | { kind: "unrenewed"; lock: LockInfo; retryAt: number }
  /** Another holds it now: saving stopped, the work kept locally.
   * `takenOver`: it was taken while this editor held it live; otherwise
   * this editor had let it lapse (a sleeping or idle tab, an outage). */
  | { kind: "lost"; lock: LockInfo; takenOver: boolean };

/** The lock's side of the network and the page. */
export interface LockDeps {
  /** `POST …/lock` with `{}`. Never rejects. */
  take(): Promise<LockReply>;
  /** The draft's etag on the server now, or `null` when it cannot be read. */
  draftEtag(): Promise<string | null>;
  /** Whether the page is in use (visible). A hidden page with nothing to
   * save stops renewing, so an idle tab does not hold a chart for ever. */
  active(): boolean;
  onLock(state: LockState): void;
}

/** The answer to one save. */
export type SaveReply =
  | { kind: "saved"; etag: string }
  /** The draft changed on the server (`409 version_conflict`): its etag
   * now, when the answer gave it. */
  | { kind: "conflict"; etag: string | null }
  /** No answer, or the server cannot take it now: tried again later. */
  | { kind: "offline"; retryAfterMs: number | null }
  /** The session ended (`401`): sign in again. */
  | { kind: "signedOut" }
  /** Refused for a reason retrying will not change (the role, an archived
   * chart, a draft too large): `message` says it. */
  | { kind: "refused"; message: string }
  /** Another holds the editing lock (`409 chart_locked`). */
  | { kind: "locked"; lock: LockInfo };

export type SaveState =
  | { kind: "saved"; at: number | null }
  /** Changes waiting for the timer, or on their way. */
  | { kind: "saving" }
  | { kind: "offline"; retryAt: number }
  | { kind: "conflict"; etag: string | null }
  | { kind: "signedOut" }
  | { kind: "refused"; message: string }
  /** Another holds the lock: nothing more is saved (`LockState` says who). */
  | { kind: "locked" };

export interface Clock {
  now(): number;
  set(run: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

/** The chart's record in the local store (`drafts.ts`). */
export interface LocalStore {
  put(draft: string, etag: string): Promise<void>;
  remove(): Promise<void>;
  /** The editor is gone: its record (if any) may now be recovered elsewhere. */
  release?(): void;
}

export interface AutosaveDeps {
  clock: Clock;
  /** The document's revision now (`status.revision`: every change raises it). */
  revision(): number;
  /** The draft as the session exports it (07 §9: never a copy of our own). */
  exportDraft(): string;
  /** Sends the draft with `If-Match: etag`. Never rejects. */
  send(draft: string, etag: string): Promise<SaveReply>;
  store: LocalStore;
  /** Told of every change of state. */
  onState(state: SaveState): void;
  /** The editing lock, when the editor holds one. */
  lock?: LockDeps;
}

export interface AutosaveStart {
  /** The etag of the draft the editor opened. */
  etag: string;
  /** The opened document's revision. */
  revision: number;
  /** Not the server's draft: recovered from the local store (`stored`),
   * or a file imported over it. Saved without waiting for an edit. */
  unsaved?: { stored: boolean };
  /** The lock as just taken or renewed (with `deps.lock`). */
  lock?: LockInfo;
}

export class Autosave {
  readonly #deps: AutosaveDeps;
  #etag: string;
  /** The revision the server holds under `#etag`; -1 for none. */
  #saved: number;
  /** The latest revision seen. */
  #latest: number;
  /** The revision in the local store (last written); null for none. */
  #local: number | null;
  #sending: number | null = null;
  #inFlight: Promise<void> = Promise.resolve();
  /** A save was due while one was in flight: send again after it. */
  #again = false;
  #saveTimer: unknown = null;
  #localTimer: unknown = null;
  #retryTimer: unknown = null;
  #attempts = 0;
  #state: SaveState;
  /** Store writes and removals, in order. */
  #chain: Promise<void> = Promise.resolve();
  #localFailed = false;
  #cache: { revision: number; draft: string } | null = null;
  #disposed = false;
  /** Waiters for the server to hold the latest revision (`flush`). */
  #waiters: ((saved: boolean) => void)[] = [];
  #lock: LockState = { kind: "none" };
  /** When the server last said the lock was this editor's. */
  #heldAt = 0;
  #renewTimer: unknown = null;
  #renewing = false;
  #lockAttempts = 0;
  /** Not renewing: the page was hidden with nothing to save. */
  #resting = false;

  constructor(deps: AutosaveDeps, start: AutosaveStart) {
    this.#deps = deps;
    this.#etag = start.etag;
    this.#latest = start.revision;
    this.#saved = start.unsaved === undefined ? start.revision : -1;
    this.#local = start.unsaved?.stored === true ? start.revision : null;
    this.#state = { kind: "saved", at: null };
    if (deps.lock !== undefined && start.lock !== undefined) {
      this.#lock = { kind: "held", lock: start.lock };
      this.#heldAt = deps.clock.now();
      this.#scheduleRenew(RENEW_MS);
    }
    if (start.unsaved === undefined) return;
    this.#schedule();
    if (!start.unsaved.stored) {
      this.#localTimer = deps.clock.set(() => {
        this.#localTimer = null;
        this.#writeLocal();
      }, LOCAL_DELAY_MS);
    }
  }

  get state(): SaveState {
    return this.#state;
  }

  /** The etag the editor holds: the base of its next save. */
  get etag(): string {
    return this.#etag;
  }

  /** Whether the server lacks the latest revision. */
  get unsaved(): boolean {
    return this.#saved !== this.#latest;
  }

  /** Whether the latest revision is in the local store (or nothing is
   * unsaved): what leaving the page loses otherwise. */
  get kept(): boolean {
    return !this.unsaved || (this.#local === this.#latest && !this.#localFailed);
  }

  /** The local store refused a write (private mode, no space). */
  get localFailed(): boolean {
    return this.#localFailed;
  }

  get lock(): LockState {
    return this.#lock;
  }

  /** The lock, while this editor holds it (or believes it does). */
  get held(): LockInfo | null {
    const l = this.#lock;
    return l.kind === "held" || l.kind === "unrenewed" ? l.lock : null;
  }

  /** The page is in use again (shown, back from the back-forward cache,
   * online): a lock not renewed lately is renewed now. */
  wake(): void {
    if (this.#disposed || this.held === null) return;
    const overdue = this.#deps.clock.now() - this.#heldAt >= RENEW_MS;
    if (this.#resting || overdue || this.#lock.kind === "unrenewed") this.#renew();
  }

  /** Stops renewing: the page lets the lock go (the caller releases it on
   * the server). Saving goes on while the page lasts. */
  letGo(): void {
    this.#clear("renew");
    this.#resting = true;
  }

  /** Another holds the lock: a refusal seen elsewhere (publishing). */
  lost(lock: LockInfo): void {
    if (this.#disposed || this.#lock.kind === "lost") return;
    this.#lose(lock);
  }

  /** Call when the document may have changed (once a frame is fine). */
  changed(): void {
    // Disposed: its session may be gone too, so nothing is read.
    if (this.#disposed) return;
    const revision = this.#deps.revision();
    if (revision === this.#latest) return;
    this.#latest = revision;
    this.#localTimer ??= this.#deps.clock.set(() => {
      this.#localTimer = null;
      this.#writeLocal();
    }, LOCAL_DELAY_MS);
    // Offline, the retry timer sends the latest: edits do not hurry it.
    if (this.#stopped() || this.#state.kind === "offline") return;
    this.#schedule();
  }

  /** Sends now (Ctrl+S, before publishing); resolves true once the server
   * holds what was the latest revision, false if it could not. */
  flush(): Promise<boolean> {
    this.changed();
    if (this.#disposed) return Promise.resolve(false);
    if (!this.unsaved && this.#sending === null) return Promise.resolve(true);
    if (this.#stopped()) return Promise.resolve(false);
    const done = new Promise<boolean>((resolve) => this.#waiters.push(resolve));
    this.#clear("save");
    this.#clear("retry");
    this.#send();
    return done;
  }

  /** Tries again now: after going offline (the browser is back online, or
   * Retry), or after a refusal the person has dealt with. */
  retry(): void {
    if (this.#disposed) return;
    this.wake();
    const k = this.#state.kind;
    if (k !== "offline" && k !== "refused") return;
    this.#clear("retry");
    this.#attempts = 0;
    this.#setState({ kind: "saving" });
    this.#send();
  }

  /**
   * Conflict resolved by keeping this editor's draft: saved over the
   * server's, whose etag is `etag` (from the conflict, or read again). A
   * draft that changed again since is another conflict.
   */
  overwrite(etag: string): void {
    if (this.#disposed || this.#state.kind !== "conflict") return;
    this.#etag = etag;
    this.#saved = -1;
    // Unsaved now, if it was not (a conflict found by a lapsed lock's
    // check): kept until the server has it.
    this.#writeLocal();
    this.#setState({ kind: "saving" });
    this.#send();
  }

  /** Writes the local store now (the page is being hidden or left). */
  keep(): Promise<void> {
    this.changed();
    if (this.unsaved) this.#writeLocal();
    return this.#chain;
  }

  /** Drops the local record: the person chose the server's draft. */
  discardLocal(): Promise<void> {
    this.#local = null;
    this.#chain = this.#chain.then(() => this.#deps.store.remove()).catch(() => {});
    return this.#chain;
  }

  /** Stops and drops its local record (the editor's document is replaced
   * at the person's request). Resolves with the etag the server holds once
   * a save on its way is answered, for the next editor to start from. */
  async abandon(): Promise<string> {
    this.#disposed = true;
    this.#clearAll();
    await Promise.all([this.discardLocal(), this.#inFlight]);
    this.#deps.store.release?.();
    return this.#etag;
  }

  /** Stops: timers cleared, a reply still on its way ignored. Unsaved
   * changes are written to the local store first. */
  dispose(): Promise<void> {
    if (this.#disposed) return this.#chain;
    const kept = this.keep().then(() => this.#deps.store.release?.());
    this.#disposed = true;
    this.#clearAll();
    return kept;
  }

  #clearAll(): void {
    this.#clear("save");
    this.#clear("local");
    this.#clear("retry");
    this.#clear("renew");
    this.#settle(false);
  }

  #stopped(): boolean {
    const k = this.#state.kind;
    return k === "conflict" || k === "signedOut" || k === "refused" || k === "locked";
  }

  #schedule(): void {
    this.#clear("save");
    this.#saveTimer = this.#deps.clock.set(() => {
      this.#saveTimer = null;
      this.#send();
    }, SAVE_DELAY_MS);
    if (this.#state.kind === "saved") this.#setState({ kind: "saving" });
  }

  #draft(): { revision: number; draft: string } {
    const revision = this.#deps.revision();
    if (this.#cache?.revision !== revision) {
      this.#cache = { revision, draft: this.#deps.exportDraft() };
    }
    this.#latest = Math.max(this.#latest, revision);
    return this.#cache;
  }

  #send(): void {
    if (this.#disposed || this.#stopped()) return;
    if (this.#sending !== null) {
      this.#again = true;
      return;
    }
    if (!this.unsaved) {
      this.#saved = this.#latest;
      this.#setState({ kind: "saved", at: this.#deps.clock.now() });
      this.#settle(true);
      return;
    }
    const { revision, draft } = this.#draft();
    this.#sending = revision;
    this.#again = false;
    if (this.#state.kind !== "offline") this.#setState({ kind: "saving" });
    this.#inFlight = this.#deps
      .send(draft, this.#etag)
      .then((reply) => this.#reply(revision, reply));
  }

  #reply(revision: number, reply: SaveReply): void {
    this.#sending = null;
    // Abandoned: the etag still follows the server, for the next editor.
    if (reply.kind === "saved") this.#etag = reply.etag;
    if (this.#disposed) return;
    const now = this.#deps.clock.now();
    // The lock was lost while this save was on its way: what it saved
    // counts, and nothing more is sent.
    if (this.#state.kind === "locked") {
      if (reply.kind === "saved") {
        this.#saved = revision;
        if (this.#local !== null && this.#local <= revision) void this.discardLocal();
      }
      this.#writeLocal();
      this.#settle(false);
      return;
    }
    switch (reply.kind) {
      case "saved": {
        this.#etag = reply.etag;
        this.#saved = revision;
        this.#attempts = 0;
        if (this.#local !== null && this.#local <= revision) void this.discardLocal();
        if (this.unsaved) {
          // A change landed meanwhile: its timer sends it, or, if the timer
          // already fired, one more save now.
          if (this.#again || this.#saveTimer === null) this.#send();
          else this.#setState({ kind: "saving" });
        } else {
          this.#clear("local");
          this.#setState({ kind: "saved", at: now });
          this.#settle(true);
        }
        return;
      }
      case "offline": {
        this.#writeLocal();
        this.#attempts += 1;
        const backoff = Math.min(SAVE_DELAY_MS * 2 ** (this.#attempts - 1), MAX_RETRY_MS);
        const wait = Math.max(backoff, reply.retryAfterMs ?? 0);
        this.#clear("save");
        this.#clear("retry");
        this.#retryTimer = this.#deps.clock.set(() => {
          this.#retryTimer = null;
          this.#send();
        }, wait);
        this.#setState({ kind: "offline", retryAt: now + wait });
        this.#settle(false);
        return;
      }
      case "conflict":
        this.#stop({ kind: "conflict", etag: reply.etag });
        return;
      case "signedOut":
        this.#stop({ kind: "signedOut" });
        return;
      case "refused":
        this.#stop({ kind: "refused", message: reply.message });
        return;
      case "locked":
        this.#lose(reply.lock);
        return;
    }
  }

  #scheduleRenew(ms: number): void {
    this.#clear("renew");
    this.#renewTimer = this.#deps.clock.set(() => {
      this.#renewTimer = null;
      this.#renew();
    }, ms);
  }

  /** Renews the lock, unless the page is idle and hidden. */
  #renew(): void {
    const lock = this.#deps.lock;
    const held = this.held;
    if (lock === undefined || held === null || this.#disposed || this.#renewing) return;
    this.#clear("renew");
    if (!lock.active() && !this.unsaved && this.#sending === null) {
      this.#resting = true;
      return;
    }
    this.#resting = false;
    this.#renewing = true;
    void lock.take().then((reply) => {
      this.#renewing = false;
      this.#renewed(held, reply);
    });
  }

  #renewed(was: LockInfo, reply: LockReply): void {
    if (this.#disposed || this.held === null) return;
    const now = this.#deps.clock.now();
    switch (reply.kind) {
      case "held": {
        this.#heldAt = now;
        this.#lockAttempts = 0;
        this.#setLock({ kind: "held", lock: reply.lock });
        if (!this.#resting) this.#scheduleRenew(RENEW_MS);
        // A renewal keeps `since`: a new one means the lock lapsed and was
        // taken again, and the draft may have changed meanwhile.
        if (reply.lock.since !== was.since) this.#checkDraft();
        return;
      }
      case "locked":
        this.#lose(reply.lock);
        return;
      case "offline": {
        this.#lockAttempts += 1;
        const wait = Math.min(SAVE_DELAY_MS * 2 ** (this.#lockAttempts - 1), RENEW_MS);
        this.#setLock({ kind: "unrenewed", lock: was, retryAt: now + wait });
        this.#scheduleRenew(wait);
        return;
      }
      case "signedOut":
        this.#stop({ kind: "signedOut" });
        return;
      case "refused":
        // The saves say why (an archived chart refuses them too).
        this.#setLock({ kind: "none" });
        return;
    }
  }

  /** After a lapse: a draft saved by another meanwhile is a conflict. */
  #checkDraft(): void {
    const base = this.#etag;
    void this.#deps.lock?.draftEtag().then((etag) => {
      if (this.#disposed || this.#stopped() || this.#sending !== null) return;
      // Saved meanwhile: that save's answer is the check.
      if (etag === null || this.#etag !== base || etag === base) return;
      this.#stop({ kind: "conflict", etag });
    });
  }

  #lose(lock: LockInfo): void {
    // Taken while this editor held it live, or after it let it lapse.
    const takenOver =
      !this.#resting && this.#deps.clock.now() - this.#heldAt < LOCK_MS && this.held !== null;
    this.#clear("renew");
    this.#setLock({ kind: "lost", lock, takenOver });
    this.#stop({ kind: "locked" });
  }

  #setLock(state: LockState): void {
    this.#lock = state;
    this.#deps.lock?.onLock(state);
  }

  #stop(state: SaveState): void {
    this.#clear("save");
    this.#clear("retry");
    this.#writeLocal();
    this.#setState(state);
    this.#settle(false);
  }

  #writeLocal(): void {
    if (this.#disposed || !this.unsaved) return;
    const { revision, draft } = this.#draft();
    if (this.#local === revision && !this.#localFailed) return;
    this.#clear("local");
    this.#local = revision;
    const etag = this.#etag;
    this.#chain = this.#chain
      .then(() => this.#deps.store.put(draft, etag))
      .then(
        () => {
          this.#localFailed = false;
        },
        () => {
          this.#localFailed = true;
          this.#setState(this.#state);
        },
      );
  }

  #clear(which: "save" | "local" | "retry" | "renew"): void {
    const handle =
      which === "save"
        ? this.#saveTimer
        : which === "local"
          ? this.#localTimer
          : which === "retry"
            ? this.#retryTimer
            : this.#renewTimer;
    if (handle !== null) this.#deps.clock.clear(handle);
    if (which === "save") this.#saveTimer = null;
    else if (which === "local") this.#localTimer = null;
    else if (which === "retry") this.#retryTimer = null;
    else this.#renewTimer = null;
  }

  #settle(saved: boolean): void {
    for (const resolve of this.#waiters.splice(0)) resolve(saved);
  }

  #setState(state: SaveState): void {
    this.#state = state;
    this.#deps.onState(state);
  }
}
