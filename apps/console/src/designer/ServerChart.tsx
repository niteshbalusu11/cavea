// The designer on a chart's server draft (07 §9, DSN-035, DSN-036): opens
// the draft, offers changes the browser kept, saves as the person works,
// says what happened when the draft changed elsewhere, keeps the work when
// the session ends, and warns before leaving with changes not saved.
// Called once in the designer page's scope; the page asks it for the
// indicator, the dialogs and publishing.
//
// The editing lock (07 §9, DSN-037, D-73): taken before the draft is read;
// held by another, the chart opens read-only, saying who, with Take over.
// The save machine renews it and reports it lost; the page then makes the
// session read-only, the work already in the store. Left (the tab closed or
// hidden for good, or another page of the console), it is released unless
// another tab of the same person holds it (`lockPeers.ts`).
import type { Designer, Session } from "@cavea/designer-core";
import { useBeforeLeave, useLocation, useNavigate } from "@solidjs/router";
import { createEffect, createSignal, on, onCleanup, Show, type Accessor, type JSX } from "solid-js";
import type { Chart, Version } from "../api/generated.ts";
import { asApiError, componentSignal } from "../api/query.ts";
import { describe, reference } from "../console/errors.ts";
import { here, signInFor } from "../console/paths.ts";
import { formatTime, msg } from "../i18n/messages.ts";
import { session as consoleSession } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { Confirm, Dialog, notify } from "../ui/overlays.tsx";
import {
  Autosave,
  type AutosaveStart,
  type LocalStore,
  type LockInfo,
  type LockReply,
  type LockState,
  type SaveReply,
  type SaveState,
} from "./autosave.ts";
import { draftStore, recoveryPrefix, type Recovery } from "./drafts.ts";
import { afterUploads } from "./images.ts";
import type { Editor } from "./editor.ts";
import { failureMessage, failureOf } from "./errors.ts";
import { lockPeers } from "./lockPeers.ts";
import { makeReadOnly } from "./readonly.ts";
import {
  attachThumbnail as attachTo,
  currentEtag,
  openChart,
  publish,
  readLock,
  releaseLock,
  saveDraft,
  takeLock,
  uploadImage,
  type ChartRef,
  type Published,
} from "./server.ts";
import { attachThumbnail, renderThumbnail } from "./thumbnail.ts";
import { localSource } from "./source.ts";

/** A server chart to open, with what the person's role allows. */
export interface ServerSetup {
  ref: ChartRef;
  /** `usr_…`: the local record is the person's own. */
  user: string;
  canWrite: boolean;
  canPublish: boolean;
  /** The role's name, for messages. */
  role: string;
  /** Why it is read-only when the role is not why: the expired licence (12 §7.6). */
  closed?: string;
}

export type Phase =
  | { kind: "loading" }
  | { kind: "failed"; title: string; detail?: string | undefined; retry: boolean }
  | { kind: "ready" };

/** What the publish dialog's run reports. */
export type PublishOutcome = Published | { kind: "notSaved" };

/** Who may change the chart in this page, and why not. */
export type Access =
  | { kind: "editing" }
  /** The role reads but does not write; `lock`: who edits, if anyone. */
  | { kind: "role"; lock: LockInfo | null }
  | { kind: "archived" }
  /** Another holds the editing lock: at open, or taken from this page
   * (`takenOver`), or after this page let it lapse. `kept`: this page's
   * unsaved changes are in the browser's store. */
  | { kind: "elsewhere"; lock: LockInfo; takenOver: boolean; kept: boolean };

export interface ServerChart {
  phase: Accessor<Phase>;
  chart: Accessor<Chart | null>;
  readOnly: Accessor<boolean>;
  /** Why it is read-only, for a refusal: the role, the archive, or who
   * holds the editing lock. */
  whyReadOnly(): string;
  load(designer: Designer): Promise<void>;
  /** A session opened from a file over the draft: saved over it. */
  adopt(session: Session): Promise<void>;
  /** Save now; says so when it cannot. */
  saveNow(): void;
  publish(): Promise<PublishOutcome>;
  indicator(): JSX.Element;
  /** The page is going: unsaved work is written to the store, before the
   * editor (whose session exports it) is disposed. */
  dispose(): void;
  dialogs(): JSX.Element;
}

/** Who holds a lock, in words: their email, else what they are. */
export function holderName(lock: LockInfo, me: string): string {
  if (lock.holder === me) return msg("lock.you");
  if (lock.email !== null) return lock.email;
  return lock.holder.startsWith("key_") ? msg("lock.key") : msg("lock.member");
}

/** The lock a replaced editor held, for the next one's start. */
const held = (info: LockInfo | null): { lock?: LockInfo } => (info === null ? {} : { lock: info });

/** The draft a publish compiled, as the editor's session had it. */
interface Published_ {
  draft: string;
  revision: number;
  floor: number | null;
}

/** When a lock was taken, as a time of day. */
const since = (l: LockInfo) => formatTime(new Date(l.since));

const realClock = {
  now: () => Date.now(),
  set: (run: () => void, ms: number) => setTimeout(run, ms),
  clear: (handle: unknown) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export function serverChart(props: {
  setup: ServerSetup;
  /** Installs a session as the open editor (replacing any). */
  install: (session: Session) => Editor;
  editor: Accessor<Editor | null>;
  onPanic: (message: string) => void;
  /** The page's uploads, which a save waits for (`ImageHost`, D-74). */
  uploads: { settled(): Promise<SaveReply | null>; refusals(): number };
}): ServerChart {
  const { ref, canWrite } = props.setup;
  const signal = componentSignal();
  const navigate = useNavigate();
  const location = useLocation();
  const drafts = draftStore(recoveryPrefix(props.setup.user, ref.workspace, ref.key));
  const [phase, setPhase] = createSignal<Phase>({ kind: "loading" });
  const [chart, setChart] = createSignal<Chart | null>(null);
  const [state, setState] = createSignal<SaveState>({ kind: "saved", at: null });
  /** Records kept by earlier editors of this chart, newest first: the
   * first is offered. */
  const [kept, setKept] = createSignal<Recovery[]>([]);
  const recovery = () => {
    const record = kept()[0];
    return record === undefined ? null : { record, newer: record.etag !== serverEtag };
  };
  /** The session ended and the work could not be stored: where sign-in is. */
  const [stranded, setStranded] = createSignal<string | null>(null);
  const [conflictOpen, setConflictOpen] = createSignal(false);
  const [leaving, setLeaving] = createSignal<(() => void) | null>(null);
  const [access, setAccess] = createSignal<Access>(
    canWrite ? { kind: "editing" } : { kind: "role", lock: null },
  );
  const [lock, setLock] = createSignal<LockState>({ kind: "none" });
  /** The dialog after the lock was lost while editing. */
  const [lostOpen, setLostOpen] = createSignal(false);
  /** The take-over confirmation, naming who loses the lock. */
  const [takeOverOpen, setTakeOverOpen] = createSignal(false);
  /** Taking over would lose changes the browser could not keep. */
  const [unkept, setUnkept] = createSignal(false);
  const peers = canWrite ? lockPeers(props.setup.user, ref.workspace, ref.key) : null;
  const who = (l: LockInfo) => holderName(l, props.setup.user);
  let autosave: Autosave | null = null;
  let designer: Designer | null = null;
  /** The draft's etag as opened: the base for a restored record. */
  let serverEtag = "";
  /** The draft as opened, as the session exports it: a record equal to it
   * holds nothing to recover. */
  let serverDraft = "";
  /** Navigating away on purpose: no guard. */
  let left = false;
  /** Each operation that installs an editor (open, restore, a file over the
   * draft) takes a number; after each wait it installs only if it is still
   * the latest and the page is still here, else it disposes its session. */
  let generation = 0;
  let gone = false;
  const stale = (mine: number) => gone || mine !== generation;

  const attach = (
    session: Session,
    start: AutosaveStart | null,
    store: LocalStore = drafts.editor(),
  ): void => {
    props.install(session);
    if (start === null) return;
    setAccess({ kind: "editing" });
    // Assigned after the constructor, which may already report a state.
    let saver: Autosave | null = null;
    saver = new Autosave(
      {
        clock: realClock,
        // The session's own: the frame loop's copy can trail it by a frame,
        // and the store is written as the page is hidden.
        revision: () => session.status().revision,
        exportDraft: () => session.export(),
        // Never a draft naming an image the server does not hold yet.
        send: afterUploads(
          props.uploads,
          () => session.export(),
          (draft, etag) => saveDraft(ref, draft, etag),
        ),
        store,
        onState: (s) => {
          // A newer editor's machine has replaced this one.
          if (autosave !== saver) return;
          // A copy: the same state again (the store failed) still updates.
          setState({ ...s });
          if (s.kind !== "conflict") return;
          // Found by a lapsed lock's check, with nothing of the person's to
          // lose: the draft as it is now, said quietly.
          if (saver?.unsaved === false) {
            notify(msg("lock.reloaded"));
            return void takeTheirs();
          }
          setConflictOpen(true);
        },
        ...(start.lock === undefined
          ? {}
          : {
              lock: {
                take: () => takeLock(ref),
                draftEtag: () => currentEtag(ref, signal).catch(() => null),
                active: () => document.visibilityState === "visible",
                onLock: (l: LockState) => {
                  if (autosave !== saver) return;
                  setLock(l);
                  if (peers !== null) peers.holding = l.kind === "held" || l.kind === "unrenewed";
                  if (l.kind === "lost") lost(session, l, saver?.unsaved === true);
                },
              },
            }),
      },
      start,
    );
    autosave = saver;
    setState(saver.state);
    setLock(saver.lock);
    if (peers !== null) peers.holding = saver.held !== null;
    // For measuring and debugging in the browser's console; not in builds.
    if (import.meta.env.DEV) Object.assign(window, { caveaAutosave: saver });
  };

  // The frame loop's revision drives the machine.
  createEffect(
    on(
      () => props.editor()?.views.status.revision,
      () => autosave?.changed(),
    ),
  );

  const fail = (e: unknown) => {
    const error = asApiError(e);
    if (signal.aborted || error.sessionLost) return;
    if (error.code === "not_found") {
      return void setPhase({
        kind: "failed",
        title: msg("serverChart.notFound", { chart: ref.key }),
        retry: false,
      });
    }
    if (error.code === "forbidden" && error.reason === "role") {
      return void setPhase({
        kind: "failed",
        title: msg("serverChart.noRole", { role: props.setup.role }),
        retry: false,
      });
    }
    setPhase({
      kind: "failed",
      title: msg("serverChart.failed"),
      detail: describe(error),
      retry: true,
    });
  };

  /** Opens text as a session; a refusal is said, never thrown. */
  const open = (text: string): Session | string => {
    try {
      return designer?.open(text) ?? "";
    } catch (error) {
      const f = failureOf(error);
      if (f === null) throw error;
      if (f.kind === "panic") {
        props.onPanic(f.message);
        return f.message;
      }
      return failureMessage(f);
    }
  };

  /** Another holds the lock now (`lost` by the save machine): the session
   * becomes read-only; the work is in the store already. */
  const lost = (session: Session, l: LockState & { kind: "lost" }, stored: boolean) => {
    makeReadOnly(session, msg("lock.refused", { who: who(l.lock) }));
    setAccess({ kind: "elsewhere", lock: l.lock, takenOver: l.takenOver, kept: stored });
    setLostOpen(true);
  };

  /** Tells the other tabs whether this page holds the lock, as its save
   * machine says (none: it does not). */
  const announce = () => {
    if (peers !== null) peers.holding = (autosave?.held ?? null) !== null;
  };

  const load = async (d: Designer): Promise<void> => {
    const mine = ++generation;
    designer = d;
    setPhase({ kind: "loading" });
    // The lock before the draft, so the draft read is the one the lock's
    // last holder left; none for a role that only reads or an archived chart.
    let taken: LockReply | null = null;
    let opened;
    try {
      opened = await openChart(ref, signal, async (c) => {
        if (canWrite && c.archived_at === null && !stale(mine)) {
          const reply = await takeLock(ref);
          taken = reply;
          // Said at once, so another tab of this person closing meanwhile
          // leaves it be; unsaid below if this page does not edit.
          if (reply.kind === "held" && peers !== null) peers.holding = true;
        }
      });
    } catch (e) {
      announce();
      return void fail(e);
    }
    if (stale(mine)) return;
    // Assigned in the callback: TypeScript cannot follow it there.
    const lockReply = taken as LockReply | null;
    if (lockReply?.kind === "offline") {
      return void setPhase({
        kind: "failed",
        title: msg("serverChart.failed"),
        detail: msg("lock.unreachable"),
        retry: true,
      });
    }
    if (stale(mine)) return;
    const session = open(opened.draft);
    if (typeof session === "string") {
      announce();
      return void setPhase({
        kind: "failed",
        title: msg("serverChart.badDraft", { message: session }),
        retry: false,
      });
    }
    setChart(opened.chart);
    serverEtag = opened.etag;
    if (!canWrite) {
      makeReadOnly(session, props.setup.closed ?? msg("readOnly.refused"));
      attach(session, null);
      setPhase({ kind: "ready" });
      // Who is editing, as far as the server tells a reader.
      const editing = await readLock(ref, signal);
      if (!stale(mine)) setAccess({ kind: "role", lock: editing });
      return;
    }
    if (opened.chart.archived_at !== null) {
      makeReadOnly(session, msg("lock.archivedRefused"));
      attach(session, null);
      setAccess({ kind: "archived" });
      return void setPhase({ kind: "ready" });
    }
    if (lockReply?.kind === "locked") {
      // Someone else is editing: read-only, with Take over. Changes kept
      // here before stay in the store, offered once this page edits.
      makeReadOnly(session, msg("lock.refused", { who: who(lockReply.lock) }));
      attach(session, null);
      setAccess({ kind: "elsewhere", lock: lockReply.lock, takenOver: false, kept: false });
      return void setPhase({ kind: "ready" });
    }
    // Changes kept by earlier editors: offered unless they are what the
    // server has.
    serverDraft = session.export();
    const offered = await offers();
    if (stale(mine)) return session.dispose();
    setKept(offered);
    attach(session, {
      etag: opened.etag,
      revision: session.status().revision,
      ...(lockReply?.kind === "held" ? { lock: lockReply.lock } : {}),
    });
    setPhase({ kind: "ready" });
  };

  /** A published version's thumbnail, made in the background: a failure
   * is a quiet note, never the publish's. */
  const thumbnail = async (version: Version, published: Published_) => {
    const e = props.editor();
    if (e === null) return;
    const images = e.images;
    const source = images === null ? undefined : (hash: string) => images.source(hash);
    const render = async (): Promise<Blob | null> => {
      // The editor's own session while it still shows what was published
      // (its compiled pieces are reused); else a session of the published
      // draft, opened for the picture and closed after.
      const same = () =>
        !e.session.disposed &&
        e.session.status().revision === published.revision &&
        e.session.floors().shown === published.floor;
      if (same()) {
        const blob = await renderThumbnail(e.session, e.canvas, source, same);
        if (blob !== "changed") return blob;
      }
      const copy = designer?.open(published.draft);
      if (copy === undefined) return null;
      try {
        if (published.floor !== null) copy.setFloor(published.floor);
        const blob = await renderThumbnail(copy, null, source);
        return blob === "changed" ? null : blob;
      } finally {
        copy.dispose();
      }
    };
    const done = await attachThumbnail<Version>({
      render,
      upload: (blob) => uploadImage(ref, blob),
      attach: (hash) => attachTo(ref, version.number, hash),
      wait: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    });
    if (gone) return;
    if (done === null) return notify(msg("thumbnail.skipped", { version: version.number }));
    setChart((c) => (c?.published_version?.id === done.id ? { ...c, published_version: done } : c));
  };

  /** The person takes the lock from whoever holds it, then edits the
   * draft as it is now; their own kept changes, if any, are offered. */
  const takeOver = async (withoutMine = false) => {
    setTakeOverOpen(false);
    setLostOpen(false);
    setUnkept(false);
    // Reopening replaces this page's session with the server's draft: its
    // unsaved work must be in the store first, else the person downloads
    // it or chooses to go on without it.
    if (autosave !== null && !withoutMine && !(await autosave.keep().then(() => autosave?.kept))) {
      return void setUnkept(true);
    }
    const reply = await takeLock(ref, true);
    if (gone) return;
    if (reply.kind === "offline") return notify(msg("lock.unreachable"), "error");
    if (reply.kind !== "held") return notify(msg("lock.takeOverFailed"), "error");
    // Ours now: said at once, so another tab of this person closing while
    // the chart reopens leaves it be (unsaid if the reopening fails).
    if (peers !== null) peers.holding = true;
    // This editor's record is let go, so the reopened page can offer it.
    await autosave?.dispose();
    autosave = null;
    if (designer !== null && !gone) await load(designer);
  };

  /** The records to offer: those no open editor holds, less any equal to
   * the server's draft (removed, if still as read) and any unreadable. */
  const offers = async (): Promise<Recovery[]> => {
    const stored = await drafts.read();
    if (stored.corrupt.length > 0) notify(msg("recovery.corrupt"));
    for (const key of stored.corrupt) await drafts.remove(key).catch(() => {});
    const offered: Recovery[] = [];
    for (const record of stored.records) {
      if (record.draft !== serverDraft) offered.push(record);
      else await drafts.claim(record, (drop) => drop()).catch(() => {});
    }
    return offered;
  };

  /** The record changed since it was offered (its editor wrote again before
   * it closed): offered again as it is now. */
  const reoffer = async (mine: number) => {
    notify(msg("recovery.changed"));
    const offered = await offers();
    if (!stale(mine)) setKept(offered);
  };

  /** One recovery answer at a time: a second click while the first is on
   * its way is ignored (it would race the first for the record). */
  let answering = false;
  const oneAtATime = (run: () => Promise<void>) => async (): Promise<void> => {
    if (answering) return;
    answering = true;
    try {
      await run();
    } finally {
      answering = false;
    }
  };

  /** The person restores the record offered; the others stay for later. */
  const restore = oneAtATime(async () => {
    const mine = ++generation;
    const r = recovery();
    if (r === null) return;
    const session = open(r.record.draft);
    if (typeof session === "string") {
      notify(msg("recovery.unreadable", { message: session }), "error");
      setKept((k) => k.slice(1));
      await drafts.claim(r.record, (drop) => drop()).catch(() => {});
      return;
    }
    // The new editor's own record takes the work, then the old one goes,
    // holding the old one's lock and only if it is still as offered.
    const store = drafts.editor();
    let stored = false;
    const copy = async () => {
      await store.put(r.record.draft, autosave?.etag ?? serverEtag);
      stored = true;
    };
    const claimed = await drafts
      .claim(r.record, async (drop) => {
        await copy();
        await drop();
      })
      .catch(() => "failed" as const);
    if (claimed === "changed" || claimed === "busy") {
      session.dispose();
      store.release?.();
      return reoffer(mine);
    }
    // No Web Locks: a copy, the original kept for later.
    if (claimed === "unlocked") await copy().catch(() => {});
    // Gone or superseded: the editor shown keeps saving, untouched, and the
    // copy is recoverable once its lock is let go.
    if (stale(mine)) {
      store.release?.();
      return session.dispose();
    }
    setKept([]);
    // Onto the draft as it is now: the person chose to replace it.
    const holding = autosave?.held ?? null;
    const etag = (await autosave?.abandon()) ?? serverEtag;
    if (stale(mine)) {
      store.release?.();
      return session.dispose();
    }
    attach(
      session,
      { etag, revision: session.status().revision, unsaved: { stored }, ...held(holding) },
      store,
    );
  });

  /** The server's draft is kept over the record offered; the next is offered. */
  const discardRecovery = oneAtATime(async () => {
    const mine = generation;
    const r = recovery();
    if (r === null) return;
    const claimed = await drafts.claim(r.record, (drop) => drop()).catch(() => "failed" as const);
    if (claimed === "changed") return reoffer(mine);
    setKept((k) => (k[0] === r.record ? k.slice(1) : k));
  });

  /** The person keeps the server's draft over theirs, after a conflict. */
  const takeTheirs = async () => {
    setConflictOpen(false);
    await autosave?.abandon();
    autosave = null;
    if (designer !== null && !gone) await load(designer);
  };

  const keepMine = async () => {
    setConflictOpen(false);
    const s = state();
    if (s.kind !== "conflict") return;
    let etag = s.etag;
    if (etag === null) {
      try {
        etag = await currentEtag(ref, signal);
      } catch (e) {
        notify(describe(asApiError(e)), "error", reference(asApiError(e)));
        return setConflictOpen(true);
      }
    }
    autosave?.overwrite(etag);
  };

  const downloadKept = () => {
    const r = recovery();
    if (r !== null) void localSource.save(r.record.draft, chart()?.name ?? ref.key);
  };

  const downloadMine = () => {
    const e = props.editor();
    const draft = e?.run((s) => s.export());
    if (e !== null && draft !== undefined) void localSource.save(draft, e.views.title);
  };

  // Leaving: the browser's warning for a tab closed or reloaded with
  // changes the server lacks, the store written when the page is hidden,
  // a guard on the console's own links, and a retry when back online.
  const beforeUnload = (e: BeforeUnloadEvent) => {
    if (autosave?.unsaved !== true) return;
    void autosave.keep();
    e.preventDefault();
    // Older browsers show the warning only when this is set.
    e.returnValue = "";
  };
  const hidden = () => {
    if (document.visibilityState === "hidden") void autosave?.keep();
    else autosave?.wake();
  };
  /** The lock is let go as the page is left, unless another tab of this
   * person holds it too: closing one of two tabs leaves it to the other. */
  const release = () => {
    if ((autosave?.held ?? null) === null || autosave === null || peers === null) return;
    const alone = peers.alone;
    peers.holding = false;
    if (!alone) return;
    autosave.letGo();
    releaseLock(ref);
  };
  const pagehide = () => {
    void autosave?.keep();
    release();
  };
  // Back from the back-forward cache: the lock is taken again.
  const pageshow = (e: PageTransitionEvent) => {
    if (!e.persisted) return;
    // Said "gone" as it was hidden: holding again, for the other tabs.
    if (peers !== null && (autosave?.held ?? null) !== null) peers.holding = true;
    autosave?.wake();
  };
  const online = () => autosave?.retry();
  window.addEventListener("beforeunload", beforeUnload);
  window.addEventListener("pagehide", pagehide);
  window.addEventListener("pageshow", pageshow);
  document.addEventListener("visibilitychange", hidden);
  window.addEventListener("online", online);
  onCleanup(() => {
    window.removeEventListener("beforeunload", beforeUnload);
    window.removeEventListener("pagehide", pagehide);
    window.removeEventListener("pageshow", pageshow);
    document.removeEventListener("visibilitychange", hidden);
    window.removeEventListener("online", online);
  });
  useBeforeLeave((e) => {
    if (left || e.defaultPrevented || autosave?.unsaved !== true) return;
    e.preventDefault();
    setLeaving(() => () => {
      left = true;
      e.retry(true);
    });
  });

  // The session ended under the editor (it expired, or ended elsewhere):
  // the work is written to the store first, then sign-in, which brings the
  // person back here, where it is offered.
  createEffect(
    on(
      () => consoleSession().kind,
      (kind) => {
        if (kind !== "signedOut" || left) return;
        left = true;
        const back = signInFor(here(location));
        void (autosave?.dispose() ?? Promise.resolve()).then(() => {
          // Not stored (no IndexedDB here, or no space): the page is the
          // only copy, so it stays until the person has downloaded it.
          if (autosave === null || autosave.kept) navigate(back, { replace: true });
          else setStranded(back);
        });
      },
    ),
  );

  const indicator = () => (
    <Show
      when={access().kind === "editing"}
      fallback={
        <AccessIndicator
          access={access()}
          role={props.setup.role}
          closed={props.setup.closed}
          who={who}
          onTakeOver={() => setTakeOverOpen(true)}
        />
      }
    >
      <SaveIndicator
        state={state()}
        unrenewed={lock().kind === "unrenewed"}
        localFailed={autosave?.localFailed === true}
        onRetry={() => autosave?.retry()}
        onResolve={() => setConflictOpen(true)}
      />
    </Show>
  );
  /** The store refused the work: read with the save state, which is
   * reported again when a store write fails. */
  const notStored = () => (state(), autosave?.localFailed === true);
  const elsewhere = () => {
    const a = access();
    return a.kind === "elsewhere" ? a : null;
  };

  const dialogs = () => (
    <>
      <Dialog
        open={recovery() !== null}
        title={msg("recovery.title")}
        heading
        // A choice: closing would leave the kept changes to be overwritten.
        onClose={() => {}}
        class="max-w-lg"
      >
        <div class="px-5 pb-5">
          <p class="text-base text-muted">
            {msg("recovery.body", { when: formatTime(new Date(recovery()?.record.at ?? 0)) })}
          </p>
          <Show when={recovery()?.newer}>
            <p class="mt-2 text-base text-warning">{msg("recovery.newer")}</p>
          </Show>
          <div class="mt-5 flex flex-wrap justify-end gap-2">
            <Button onClick={downloadKept} icon="download">
              {msg("conflict.download")}
            </Button>
            <Button variant="danger" onClick={() => void discardRecovery()}>
              {msg("recovery.discard")}
            </Button>
            <Button variant="primary" onClick={() => void restore()}>
              {msg("recovery.restore")}
            </Button>
          </div>
        </div>
      </Dialog>
      <Dialog
        open={conflictOpen()}
        title={msg("conflict.title")}
        heading
        onClose={() => setConflictOpen(false)}
        class="max-w-lg"
      >
        <div class="px-5 pb-5">
          <p class="text-base text-muted">{msg("conflict.body")}</p>
          <div class="mt-5 flex flex-wrap justify-end gap-2">
            <Button onClick={downloadMine} icon="download">
              {msg("conflict.download")}
            </Button>
            <Button variant="danger" onClick={() => void takeTheirs()}>
              {msg("conflict.takeTheirs")}
            </Button>
            <Button variant="primary" onClick={() => void keepMine()}>
              {msg("conflict.keepMine")}
            </Button>
          </div>
        </div>
      </Dialog>
      <Dialog
        open={lostOpen() && elsewhere() !== null}
        title={msg(elsewhere()?.takenOver === true ? "lock.lostTitle" : "lock.lapsedTitle", {
          who: who(elsewhere()?.lock ?? { holder: "", email: null, since: "" }),
        })}
        heading
        onClose={() => setLostOpen(false)}
        class="max-w-lg"
      >
        <div class="px-5 pb-5">
          <p class="text-base text-muted">{msg("lock.lostBody")}</p>
          <Show when={elsewhere()?.kept}>
            <p
              class="mt-2 text-base"
              classList={{ "text-fg": !notStored(), "text-danger": notStored() }}
            >
              {msg(notStored() ? "lock.lostNotKept" : "lock.lostKept")}
            </p>
          </Show>
          <div class="mt-5 flex flex-wrap justify-end gap-2">
            <Show when={elsewhere()?.kept}>
              <Button onClick={downloadMine} icon="download">
                {msg("conflict.download")}
              </Button>
            </Show>
            <Button onClick={() => setTakeOverOpen(true)}>{msg("lock.takeOver")}</Button>
            <Button variant="primary" onClick={() => setLostOpen(false)}>
              {msg("action.close")}
            </Button>
          </div>
        </div>
      </Dialog>
      <Confirm
        open={takeOverOpen() && elsewhere() !== null}
        title={msg("lock.confirmTitle", {
          who: who(elsewhere()?.lock ?? { holder: "", email: null, since: "" }),
        })}
        description={msg("lock.confirmBody", {
          who: who(elsewhere()?.lock ?? { holder: "", email: null, since: "" }),
        })}
        confirm={msg("lock.takeOverConfirm")}
        variant="danger"
        onConfirm={() => void takeOver()}
        onClose={() => setTakeOverOpen(false)}
      />
      <Confirm
        open={unkept()}
        title={msg("lock.unkeptTitle")}
        description={msg("lock.unkeptBody")}
        confirm={msg("lock.unkeptConfirm")}
        variant="danger"
        onConfirm={() => void takeOver(true)}
        onClose={() => setUnkept(false)}
      >
        <div class="mt-4">
          <Button onClick={downloadMine} icon="download">
            {msg("conflict.download")}
          </Button>
        </div>
      </Confirm>
      <Confirm
        open={stranded() !== null}
        blocking
        title={msg("stranded.title")}
        description={msg("stranded.body")}
        confirm={msg("stranded.signIn")}
        variant="danger"
        onConfirm={() => {
          const back = stranded();
          if (back !== null) navigate(back, { replace: true });
        }}
        onClose={() => {}}
      >
        <div class="mt-4">
          <Button onClick={downloadMine} icon="download">
            {msg("conflict.download")}
          </Button>
        </div>
      </Confirm>
      <Confirm
        open={leaving() !== null}
        title={msg("leave.title")}
        description={msg("leave.body")}
        confirm={msg("leave.confirm")}
        variant="danger"
        onConfirm={() => {
          const go = leaving();
          setLeaving(null);
          void (autosave?.keep() ?? Promise.resolve()).then(() => go?.());
        }}
        onClose={() => setLeaving(null)}
      />
    </>
  );

  return {
    phase,
    chart,
    readOnly: () => access().kind !== "editing",
    whyReadOnly() {
      const a = access();
      if (a.kind === "archived") return msg("lock.archivedRefused");
      if (a.kind === "elsewhere") return msg("lock.refused", { who: who(a.lock) });
      return props.setup.closed ?? msg("readOnly.refused");
    },
    load,
    async adopt(session) {
      // Read-only: nothing replaces the draft (the page refuses first).
      if (access().kind !== "editing") return session.dispose();
      const mine = ++generation;
      const holding = autosave?.held ?? null;
      const etag = (await autosave?.abandon()) ?? serverEtag;
      if (stale(mine)) return session.dispose();
      attach(session, {
        etag,
        revision: session.status().revision,
        unsaved: { stored: false },
        ...held(holding),
      });
    },
    saveNow() {
      void autosave?.flush().then((saved) => {
        if (!saved && autosave?.state.kind !== "conflict") notify(msg("save.failed"), "error");
      });
    },
    async publish() {
      if (autosave === null || !(await autosave.flush())) return { kind: "notSaved" };
      // What is published: the draft as just saved, which the person may
      // go on editing while the server compiles it.
      const s = props.editor()?.session;
      const published =
        s === undefined || s.disposed
          ? null
          : { draft: s.export(), revision: s.status().revision, floor: s.floors().shown };
      const result = await publish(ref, autosave.etag, signal);
      if (result.kind === "published") {
        setChart((c) => (c === null ? c : { ...c, published_version: result.version }));
        // A new version, or one published again before it had a picture.
        if (result.version.thumbnail === null && published !== null) {
          void thumbnail(result.version, published);
        }
      }
      if (result.kind === "locked") autosave.lost(result.lock);
      return result;
    },
    indicator,
    dispose: () => {
      gone = true;
      // Another page of the console: the lock goes as when the tab closes.
      release();
      peers?.close();
      void autosave?.dispose();
    },
    dialogs,
  };
}

function AccessIndicator(props: {
  access: Access;
  role: string;
  closed: string | undefined;
  who: (lock: LockInfo) => string;
  onTakeOver: () => void;
}) {
  const text = () => {
    const a = props.access;
    switch (a.kind) {
      case "editing":
        return "";
      case "role":
        return a.lock === null
          ? msg("save.readOnly")
          : msg("lock.roleEditing", { who: props.who(a.lock) });
      case "archived":
        return msg("lock.archived");
      case "elsewhere":
        return msg(a.takenOver ? "lock.takenOver" : "lock.editing", {
          who: props.who(a.lock),
          since: since(a.lock),
        });
    }
  };
  const title = () =>
    props.access.kind === "role"
      ? (props.closed ?? msg("save.readOnlyWhy", { role: props.role }))
      : undefined;
  return (
    <span class="flex min-w-0 items-center gap-2">
      <span
        class="truncate rounded-sm bg-warning-soft px-1.5 text-xs text-warning"
        role="status"
        title={title()}
      >
        {text()}
      </span>
      <Show when={props.access.kind === "elsewhere"}>
        <button
          type="button"
          class="shrink-0 text-sm text-accent underline"
          onClick={() => props.onTakeOver()}
        >
          {msg("lock.takeOver")}
        </button>
      </Show>
    </span>
  );
}

function SaveIndicator(props: {
  state: SaveState;
  /** The editing lock could not be renewed: the server is unreachable. */
  unrenewed: boolean;
  localFailed: boolean;
  onRetry: () => void;
  onResolve: () => void;
}) {
  const text = () => {
    const s = props.state;
    switch (s.kind) {
      case "saved":
        if (props.unrenewed) return msg("lock.unrenewed");
        return s.at === null ? "" : msg("save.savedAt", { time: formatTime(new Date(s.at)) });
      case "saving":
        return props.unrenewed ? msg("lock.unrenewed") : msg("save.saving");
      case "offline":
        if (props.localFailed) return msg("save.offlineNotKept");
        return props.unrenewed ? msg("lock.unrenewed") : msg("save.offline");
      case "conflict":
        return msg("save.conflict");
      case "signedOut":
        return msg("save.signedOut");
      case "refused":
        return msg("save.refused", { message: s.message });
      case "locked":
        return "";
    }
  };
  const warn = () =>
    props.unrenewed || (props.state.kind !== "saved" && props.state.kind !== "saving");
  return (
    <span class="flex min-w-0 items-center gap-2">
      <span
        class="truncate text-sm"
        classList={{ "text-faint": !warn(), "text-warning": warn() }}
        role="status"
      >
        {text()}
      </span>
      <Show when={props.state.kind === "offline" || props.state.kind === "refused"}>
        <button type="button" class="text-sm text-accent underline" onClick={() => props.onRetry()}>
          {msg("save.retry")}
        </button>
      </Show>
      <Show when={props.state.kind === "conflict"}>
        <button
          type="button"
          class="text-sm text-accent underline"
          onClick={() => props.onResolve()}
        >
          {msg("save.resolve")}
        </button>
      </Show>
    </span>
  );
}
