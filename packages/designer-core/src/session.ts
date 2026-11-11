import type { Bindings, Handle, WasmSource } from "./bindings.ts";
import { Calls, type CallTable, type Failure } from "./generated/calls.ts";

/**
 * Why a call threw: a `Failure` from the module (`command`: the edit core
 * refused it and nothing changed; `import`: the document cannot be opened;
 * `arguments`: they do not match the call), or one of the package's own:
 * `disposed` (the session was disposed) and `panic` (the module trapped: a
 * bug in the edit core; the module, which the page cannot instantiate a
 * second time, is unusable from then on and every session of it throws
 * `panic` until the page is reloaded).
 */
export type DesignerFailure = Failure | { kind: "disposed" } | { kind: "panic"; message: string };

export class DesignerError extends Error {
  readonly failure: DesignerFailure;

  constructor(failure: DesignerFailure) {
    super(describe(failure));
    this.name = "DesignerError";
    this.failure = failure;
  }
}

function describe(failure: DesignerFailure): string {
  switch (failure.kind) {
    case "command":
    case "import":
      return `${failure.kind} refused: ${failure.error.code}`;
    case "arguments":
    case "encode":
    case "panic":
      return `${failure.kind}: ${failure.message}`;
    case "unknownCall":
      return `no call ${failure.name}`;
    case "noChart":
    case "disposed":
      return failure.kind;
  }
}

type Reply = { ok: unknown } | { err: Failure };

/** The loaded module, shared by its sessions. */
interface Module {
  bindings: Bindings;
  /** Set when the module trapped. */
  broken: string | null;
}

/** A loaded module: open a chart in it as many times as needed. */
export interface Designer {
  /**
   * Opens chart document JSON (a draft from the server, or a file being
   * imported) as a new session. Throws a `DesignerError` with an `import`
   * failure when it cannot be opened.
   */
  open(text: string): Session;
}

export interface LoadOptions {
  /** The WASM: bytes, a compiled module, a response or a URL. Default:
   * `../wasm/cavea_designer_bg.wasm` beside this package. */
  wasm?: WasmSource;
  /** The URL of the module's JavaScript bindings. Default:
   * `../wasm/cavea_designer.js` beside this package. A bundler copies or
   * serves both files as assets. */
  bindings?: string | URL;
}

/** The state of each instantiated module, by its bindings: the bindings
 * keep one instance, which every `Designer` loaded from them shares. */
const modules = new WeakMap<Bindings, Module>();

/**
 * Loads and instantiates the module, once per page: later calls share the
 * instance, and a module broken by a trap stays broken.
 */
export async function loadDesigner(options: LoadOptions = {}): Promise<Designer> {
  const url = options.bindings ?? new URL("../wasm/cavea_designer.js", import.meta.url);
  // A path resolved at run time: the package type-checks without the build.
  const bindings = (await import(/* @vite-ignore */ String(url))) as Bindings;
  await bindings.default({
    module_or_path: options.wasm ?? new URL("../wasm/cavea_designer_bg.wasm", import.meta.url),
  });
  const module = modules.get(bindings) ?? { bindings, broken: null };
  modules.set(bindings, module);
  return {
    open(text: string): Session {
      const made: { handle?: Handle } = {};
      try {
        settle(module, () => {
          made.handle = new bindings.Designer();
          return made.handle.import(text);
        });
      } catch (error) {
        if (module.broken === null) {
          made.handle?.free();
        }
        throw error;
      }
      if (made.handle === undefined) {
        throw new DesignerError({ kind: "panic", message: "no session was made" });
      }
      return new Session(module, made.handle);
    },
  };
}

/**
 * The arguments as JSON, or an `arguments` failure for what JSON cannot
 * carry: NaN and infinities (`JSON.stringify` would write `null`, which a
 * nullable argument takes as a value), a `BigInt`, a cycle.
 */
function encode(args: unknown[]): string {
  // What was thrown is never inspected: a getter in the arguments may have
  // thrown anything, even a value that throws when read.
  let message = "not representable as JSON";
  try {
    return JSON.stringify(args, (_key, value: unknown) => {
      if (typeof value === "number" && !Number.isFinite(value)) {
        message = `${value} is not a finite number`;
        throw new Error(message);
      }
      return value;
    });
  } catch {
    throw new DesignerError({ kind: "arguments", message });
  }
}

/** Parses a reply, or turns what the module threw into a `panic`. */
function settle(module: Module, run: () => string, onPanic: () => void = () => {}): unknown {
  if (module.broken !== null) {
    throw new DesignerError({ kind: "panic", message: module.broken });
  }
  let text: string;
  try {
    text = run();
  } catch (error) {
    module.broken = error instanceof Error ? error.message : String(error);
    onPanic();
    throw new DesignerError({ kind: "panic", message: module.broken });
  }
  const reply = JSON.parse(text) as Reply;
  if ("err" in reply) {
    throw new DesignerError(reply.err);
  }
  return reply.ok;
}

/**
 * A designer session: the open chart with its undo history, selection,
 * tool and views (07 §2.3). Made by `Designer.open`. Every method is one
 * call of `crates/cavea-designer/src/calls.rs`; a refused call throws a
 * `DesignerError` and changes nothing.
 *
 * The frame loop: once per animation frame call `changed()` and re-read
 * only the views it names; in idle time call `validate(budget)` until it
 * reports `done`. `dispose()` frees the session; calls after it throw
 * `disposed`.
 */
export class Session extends Calls {
  readonly #module: Module;
  #handle: Handle | null;

  /** Use `Designer.open`. */
  constructor(module: Module, handle: Handle) {
    super();
    this.#module = module;
    this.#handle = handle;
  }

  protected call<M extends keyof CallTable>(
    method: M,
    args: CallTable[M]["args"],
  ): CallTable[M]["returns"] {
    const handle = this.#handle;
    if (this.#module.broken !== null) {
      throw new DesignerError({ kind: "panic", message: this.#module.broken });
    }
    if (handle === null) {
      throw new DesignerError({ kind: "disposed" });
    }
    const json = encode(args);
    return settle(
      this.#module,
      () => handle.call(method, json),
      () => {
        // After a trap the session's memory may be inconsistent: never
        // touch it again, not even to free it.
        this.#handle = null;
      },
    ) as CallTable[M]["returns"];
  }

  /** Frees the session. Idempotent. */
  dispose(): void {
    if (this.#module.broken === null) {
      this.#handle?.free();
    }
    this.#handle = null;
  }

  get disposed(): boolean {
    return this.#handle === null;
  }
}
