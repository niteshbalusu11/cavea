// The designer on a chart's server draft (07 §9, DSN-035, DSN-036): opens
// the draft, offers changes the browser kept, saves as the person works,
// says what happened when the draft changed elsewhere, keeps the work when
// the session ends, and warns before leaving with changes not saved.
// Called once in the designer page's scope; the page asks it for the
// indicator, the dialogs and publishing.
import type { Designer, Session } from "@cavea/designer-core";
import { useBeforeLeave, useLocation, useNavigate } from "@solidjs/router";
import { createEffect, createSignal, on, onCleanup, Show, type Accessor, type JSX } from "solid-js";
import type { Chart } from "../api/generated.ts";
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
  type SaveReply,
  type SaveState,
} from "./autosave.ts";
import { draftStore, recoveryPrefix, type Recovery } from "./drafts.ts";
import type { Editor } from "./editor.ts";
import { failureMessage, failureOf } from "./errors.ts";
import { makeReadOnly } from "./readonly.ts";
import {
  currentEtag,
  openChart,
  publish,
  saveDraft,
  type ChartRef,
  type Published,
} from "./server.ts";
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
}

export type Phase =
  | { kind: "loading" }
  | { kind: "failed"; title: string; detail?: string | undefined; retry: boolean }
  | { kind: "ready" };

/** What the publish dialog's run reports. */
export type PublishOutcome = Published | { kind: "notSaved" };

export interface ServerChart {
  phase: Accessor<Phase>;
  chart: Accessor<Chart | null>;
  readOnly: boolean;
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
  /** Before a save: `null` once the images it names are on the server,
   * else what the save reports meanwhile (`ImageHost.settled`). */
  settled: () => Promise<SaveReply | null>;
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
        send: async (draft, etag) => (await props.settled()) ?? saveDraft(ref, draft, etag),
        store,
        onState: (s) => {
          // A newer editor's machine has replaced this one.
          if (autosave !== saver) return;
          // A copy: the same state again (the store failed) still updates.
          setState({ ...s });
          if (s.kind === "conflict") setConflictOpen(true);
        },
      },
      start,
    );
    autosave = saver;
    setState(saver.state);
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

  const load = async (d: Designer): Promise<void> => {
    const mine = ++generation;
    designer = d;
    setPhase({ kind: "loading" });
    let opened;
    try {
      opened = await openChart(ref, signal);
    } catch (e) {
      return void fail(e);
    }
    if (stale(mine)) return;
    const session = open(opened.draft);
    if (typeof session === "string") {
      return void setPhase({
        kind: "failed",
        title: msg("serverChart.badDraft", { message: session }),
        retry: false,
      });
    }
    setChart(opened.chart);
    serverEtag = opened.etag;
    if (!canWrite) {
      makeReadOnly(session);
      attach(session, null);
      return void setPhase({ kind: "ready" });
    }
    // Changes kept by earlier editors: offered unless they are what the
    // server has.
    serverDraft = session.export();
    const offered = await offers();
    if (stale(mine)) return session.dispose();
    setKept(offered);
    attach(session, { etag: opened.etag, revision: session.status().revision });
    setPhase({ kind: "ready" });
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
    const etag = (await autosave?.abandon()) ?? serverEtag;
    if (stale(mine)) {
      store.release?.();
      return session.dispose();
    }
    attach(session, { etag, revision: session.status().revision, unsaved: { stored } }, store);
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
  };
  const online = () => autosave?.retry();
  window.addEventListener("beforeunload", beforeUnload);
  window.addEventListener("pagehide", hidden);
  document.addEventListener("visibilitychange", hidden);
  window.addEventListener("online", online);
  onCleanup(() => {
    window.removeEventListener("beforeunload", beforeUnload);
    window.removeEventListener("pagehide", hidden);
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
      when={canWrite}
      fallback={
        <span
          class="rounded-sm bg-warning-soft px-1.5 text-xs text-warning"
          title={msg("save.readOnlyWhy", { role: props.setup.role })}
        >
          {msg("save.readOnly")}
        </span>
      }
    >
      <SaveIndicator
        state={state()}
        localFailed={autosave?.localFailed === true}
        onRetry={() => autosave?.retry()}
        onResolve={() => setConflictOpen(true)}
      />
    </Show>
  );

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
    readOnly: !canWrite,
    load,
    async adopt(session) {
      // Read-only: nothing replaces the draft (the page refuses first).
      if (!canWrite) return session.dispose();
      const mine = ++generation;
      const etag = (await autosave?.abandon()) ?? serverEtag;
      if (stale(mine)) return session.dispose();
      attach(session, { etag, revision: session.status().revision, unsaved: { stored: false } });
    },
    saveNow() {
      void autosave?.flush().then((saved) => {
        if (!saved && autosave?.state.kind !== "conflict") notify(msg("save.failed"), "error");
      });
    },
    async publish() {
      if (autosave === null || !(await autosave.flush())) return { kind: "notSaved" };
      const result = await publish(ref, autosave.etag, signal);
      if (result.kind === "published") {
        setChart((c) => (c === null ? c : { ...c, published_version: result.version }));
      }
      return result;
    },
    indicator,
    dispose: () => {
      gone = true;
      void autosave?.dispose();
    },
    dialogs,
  };
}

function SaveIndicator(props: {
  state: SaveState;
  localFailed: boolean;
  onRetry: () => void;
  onResolve: () => void;
}) {
  const text = () => {
    const s = props.state;
    switch (s.kind) {
      case "saved":
        return s.at === null ? "" : msg("save.savedAt", { time: formatTime(new Date(s.at)) });
      case "saving":
        return msg("save.saving");
      case "offline":
        return props.localFailed ? msg("save.offlineNotKept") : msg("save.offline");
      case "conflict":
        return msg("save.conflict");
      case "signedOut":
        return msg("save.signedOut");
      case "refused":
        return msg("save.refused", { message: s.message });
    }
  };
  const warn = () => props.state.kind !== "saved" && props.state.kind !== "saving";
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
