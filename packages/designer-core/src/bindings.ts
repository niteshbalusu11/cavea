// The shape of the module `cargo xtask build-designer` writes to ../wasm
// (wasm-bindgen output, not committed): `crates/cavea-designer/src/lib.rs`'s
// exports. Declared here so that the package type-checks without the
// build; the boundary test checks it against the built module.

/** What the module can be instantiated from. */
export type WasmSource = Response | BufferSource | WebAssembly.Module | string | URL;

export interface Bindings {
  default(init?: { module_or_path: WasmSource | Promise<WasmSource> }): Promise<unknown>;
  Designer: new () => Handle;
  Canvas: new (canvas: HTMLCanvasElement) => CanvasHandle;
  /** A preview's sample state: a SNAPSHOT frame for a public artefact, or empty. */
  sampleState(chart: Uint8Array, sold: number, seq: number): Uint8Array;
}

/** `cavea_designer::Designer`: every reply is `{"ok": …}` or `{"err": Failure}`. */
export interface Handle {
  import(text: string): string;
  call(method: string, args: string): string;
  /** The open chart's public artefact, or empty when it does not compile. */
  preview(): Uint8Array;
  free(): void;
}

/** `cavea_designer::canvas::Canvas`: a WebGL2 canvas drawing a session. */
export interface CanvasHandle {
  setView(
    centreX: number,
    centreY: number,
    zoom: number,
    width: number,
    height: number,
    dpr: number,
  ): boolean;
  setTheme(dark: boolean): void;
  /** Draws `designer`'s session if anything changed; whether to draw again. */
  draw(designer: Handle, timeMs: number): boolean;
  /** Text requests and errors, as JSON. */
  takeEvents(): string;
  setText(id: number, width: number, height: number, px: number, pixels: Uint8Array): boolean;
  setGlyphAtlas(rgba: Uint8Array): boolean;
  contextLost(): void;
  contextRestored(): boolean;
  isContextLost(): boolean;
  /** Draw calls, seats drawn, level of detail, pixel ratio, pieces compiled, merges. */
  frameStats(): Float64Array;
  /** `[min, max]` CSS pixels per chart unit, or empty. */
  zoomLimits(): Float64Array;
  free(): void;
}
