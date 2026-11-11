// The one frame loop that owns a session's views (07 §2.3, D-40). Once per
// animation frame it asks `changed()` and re-reads only the views it names
// that something shows; the shell's components read what it wrote and never
// call a view themselves. In idle time it runs validation, never during a
// gesture. Framework free: it writes into a `Sink` (Solid stores in the app,
// a recorder in the tests).
import type {
  CategoriesView,
  FloorsView,
  HistoryView,
  InspectorView,
  LayersPage,
  NumberingView,
  Session,
  SelectionView,
  StatusView,
  ToolView,
  ValidationView,
  ViewName,
} from "@cavea/designer-core";

export interface Views {
  history: HistoryView;
  title: string;
  floors: FloorsView;
  status: StatusView;
  selection: SelectionView;
  tool: ToolView;
  inspector: InspectorView;
  categories: CategoriesView;
  validation: ValidationView;
  layers: LayersPage;
  /** Read only while the numbering panel is open (`FrameLoop.watch`). */
  numbering: NumberingView | null;
}

export interface Sink {
  set<K extends keyof Views>(view: K, value: Views[K]): void;
}

/** The session calls the loop makes. */
export type LoopSession = Pick<
  Session,
  | "changed"
  | "history"
  | "title"
  | "floors"
  | "status"
  | "selectionView"
  | "toolView"
  | "inspector"
  | "categories"
  | "validation"
  | "layers"
  | "numbering"
  | "validate"
>;

/** Views read whenever they change. `numbering` is read only while the
 * numbering panel shows (`watch`); `overlay` and `cursor` (the canvas,
 * DSN-004) and `guides` (DSN-031) are never read yet. */
const ON_DEMAND = { numbering: "numbering" } as const satisfies Partial<
  Record<ViewName, keyof Views>
>;
type OnDemand = keyof typeof ON_DEMAND;

const READ: Partial<Record<ViewName, keyof Views>> = {
  history: "history",
  title: "title",
  floors: "floors",
  status: "status",
  selection: "selection",
  tool: "tool",
  inspector: "inspector",
  categories: "categories",
  validation: "validation",
  layers: "layers",
};

export interface IdleDeadline {
  timeRemaining(): number;
}

export interface Scheduler {
  frame(run: () => void): () => void;
  idle(run: (deadline: IdleDeadline) => void): () => void;
  now(): number;
  /** Calls `run` when the page is hidden or shown; returns the unsubscribe. */
  visibility(run: (hidden: boolean) => void): () => void;
  hidden(): boolean;
}

/** Validation budget per idle millisecond: seat positions compiled (07 §10;
 * about 1,250 per ms in the full pass on `stadium-100k`, natively). */
const SEATS_PER_MS = 1_000;
/** Edits must have settled this long before validation starts again. */
const SETTLE_MS = 300;
/** After this long without an edit a piece longer than the idle period
 * (the full pass) runs anyway: the pause is accepted (07 §10). */
const PAUSE_OK_MS = 1_000;

export class FrameLoop {
  readonly #session: LoopSession;
  readonly #sink: Sink;
  readonly #at: Scheduler;
  readonly #onError: (error: unknown) => void;
  #window: [number, number] = [0, 0];
  #windowDirty = false;
  /** Views read on demand: shown by something now, and to read next frame. */
  #watched = new Set<OnDemand>();
  #dirty = new Set<OnDemand>();
  #cancelFrame: (() => void) | null = null;
  #cancelIdle: (() => void) | null = null;
  #unwatch: () => void = () => {};
  #revision = -1;
  #lastEdit = 0;
  #gesture = false;
  #complete = true;
  #next = 0;
  #disposed = false;

  /** `onError`: a call threw (the module trapped); the loop has stopped. */
  constructor(
    session: LoopSession,
    sink: Sink,
    scheduler: Scheduler,
    onError: (error: unknown) => void = () => {},
  ) {
    this.#session = session;
    this.#sink = sink;
    this.#at = scheduler;
    this.#onError = onError;
  }

  #guard(run: () => void): void {
    try {
      run();
    } catch (error) {
      this.dispose();
      this.#onError(error);
    }
  }

  /** Starts the frames; they pause while the page is hidden. */
  start(): void {
    this.#unwatch = this.#at.visibility((hidden) => (hidden ? this.#pause() : this.#request()));
    if (!this.#at.hidden()) this.#request();
  }

  dispose(): void {
    this.#disposed = true;
    this.#pause();
    this.#cancelIdle?.();
    this.#cancelIdle = null;
    this.#unwatch();
  }

  /** The rows of the layers tree something shows; read on the next frame. */
  setLayersWindow(start: number, count: number): void {
    if (start === this.#window[0] && count === this.#window[1]) return;
    this.#window = [start, count];
    this.#windowDirty = true;
  }

  /** Starts or stops reading a view read on demand (`numbering`): read on
   * the next frame when started; while stopped it costs nothing. */
  watch(view: OnDemand, on: boolean): void {
    if (on === this.#watched.has(view)) return;
    if (on) {
      this.#watched.add(view);
      this.#dirty.add(view);
    } else {
      this.#watched.delete(view);
      this.#sink.set(view, null);
    }
  }

  /** Reads what changed now rather than at the next frame: for input that
   * acts on what a view shows, when keys come faster than frames. */
  flush(): void {
    if (!this.#disposed) this.#guard(() => void this.tick());
  }

  /** One frame: the views read, for the tests. */
  tick(): (keyof Views)[] {
    const read: (keyof Views)[] = [];
    const names = new Set(this.#session.changed());
    if (this.#windowDirty) names.add("layers");
    this.#windowDirty = false;
    for (const name of this.#dirty) names.add(name);
    this.#dirty.clear();
    for (const name of names) {
      const view =
        READ[name] ??
        (name in ON_DEMAND && this.#watched.has(name as OnDemand)
          ? ON_DEMAND[name as OnDemand]
          : undefined);
      if (view === undefined) continue;
      read.push(view);
      this.#read(view);
    }
    return read;
  }

  #read(view: keyof Views): void {
    const s = this.#session;
    const sink = this.#sink;
    switch (view) {
      case "history": {
        const h = s.history();
        this.#gesture = h.gesture;
        return sink.set(view, h);
      }
      case "title":
        return sink.set(view, s.title());
      case "floors":
        return sink.set(view, s.floors());
      case "status": {
        const st = s.status();
        if (st.revision !== this.#revision) {
          this.#revision = st.revision;
          this.#lastEdit = this.#at.now();
        }
        return sink.set(view, st);
      }
      case "selection":
        return sink.set(view, s.selectionView());
      case "tool":
        return sink.set(view, s.toolView());
      case "inspector":
        return sink.set(view, s.inspector());
      case "categories":
        return sink.set(view, s.categories());
      case "layers":
        return sink.set(view, s.layers(this.#window[0], this.#window[1]));
      case "numbering":
        return sink.set(view, s.numbering());
      case "validation": {
        const v = s.validation();
        this.#complete = v.complete;
        if (!v.complete) this.#scheduleIdle();
        return sink.set(view, v);
      }
    }
  }

  #request(): void {
    if (this.#disposed || this.#cancelFrame !== null) return;
    this.#cancelFrame = this.#at.frame(() => {
      this.#cancelFrame = null;
      this.#guard(() => {
        this.tick();
        this.#request();
      });
    });
  }

  #pause(): void {
    this.#cancelFrame?.();
    this.#cancelFrame = null;
  }

  #scheduleIdle(): void {
    if (this.#disposed || this.#cancelIdle !== null) return;
    this.#cancelIdle = this.#at.idle((deadline) => {
      this.#cancelIdle = null;
      this.#guard(() => this.validateIdle(deadline));
    });
  }

  /** Validation pieces that fit in `deadline`, after edits have settled
   * and outside a gesture; reschedules itself until the list is complete.
   * Public for the tests. */
  validateIdle(deadline: IdleDeadline): void {
    if (this.#disposed || this.#complete) return;
    const quiet = this.#at.now() - this.#lastEdit;
    if (this.#gesture || quiet < SETTLE_MS) return this.#scheduleIdle();
    let remaining = deadline.timeRemaining();
    while (remaining > 1) {
      const budget = Math.max(1, Math.floor(remaining * SEATS_PER_MS));
      if (this.#next > budget && quiet < PAUSE_OK_MS) break;
      const progress = this.#session.validate(budget);
      this.#next = progress.next;
      if (progress.done) return;
      remaining = deadline.timeRemaining();
    }
    this.#scheduleIdle();
  }
}

/** The browser's frames, idle callbacks (with a timer where there are
 * none) and visibility. */
export const browserScheduler: Scheduler = {
  frame(run) {
    const id = requestAnimationFrame(run);
    return () => cancelAnimationFrame(id);
  },
  idle(run) {
    if (typeof requestIdleCallback === "function") {
      const id = requestIdleCallback(run, { timeout: 2_000 });
      return () => cancelIdleCallback(id);
    }
    const id = setTimeout(() => {
      const start = performance.now();
      run({ timeRemaining: () => Math.max(0, 12 - (performance.now() - start)) });
    }, 50);
    return () => clearTimeout(id);
  },
  now: () => performance.now(),
  visibility(run) {
    const listener = () => run(document.hidden);
    document.addEventListener("visibilitychange", listener);
    return () => document.removeEventListener("visibilitychange", listener);
  },
  hidden: () => document.hidden,
};
