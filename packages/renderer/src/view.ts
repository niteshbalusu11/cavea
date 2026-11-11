// The part of the WASM `View` the SDK uses, as a type the generated class
// satisfies (checked where `browser.ts` constructs it), so tests can drive the
// SDK with a recording fake instead of WebGL.

export interface EngineView {
  readonly needFrame: boolean;
  free(): void;
  loadChart(ccf: Uint8Array): void;
  setGlyphAtlas(rgba: Uint8Array): void;
  /** The raster for a `textRequest`: width, height, em in device pixels, coverage bytes. */
  setText(id: number, width: number, height: number, px: number, pixels: Uint8Array): void;
  /** A built-in string the map draws (`soldOut`). */
  setMessage(key: string, text: string): void;
  /** One SNAPSHOT, DELTA or PRIVATE frame; `false` for frames it does not take. */
  applyFrame(frame: Uint8Array): boolean;
  /** Label state, converted in one call; each returns the unknown labels. */
  setSnapshotLabels(seq: number, unavailable: string[]): string[];
  applyDeltaLabels(from: number, to: number, unavailable: string[], available: string[]): string[];
  setPrivateLabels(labels: string[], expiresAt: number): string[];
  holdOk(request: number): void;
  holdFailed(request: number, code: string, labels: string[], reasons: string[]): void;
  select(labels: string[]): void;
  deselect(labels: string[]): void;
  clearSelection(): void;
  setRules(max?: number | null, min?: number | null): void;
  setManualSession(manual: boolean): void;
  setTheme(name: string, tokens: string[]): void;
  setGestureHandling(mode: number): void;
  setReducedMotion(on: boolean): void;
  /** `rect`: x, y, width, height in chart units; else `label`; else the chart. */
  flyTo(
    rect: Float32Array | null | undefined,
    label: string | null | undefined,
    section: boolean,
    instant: boolean,
  ): void;
  /** JSON: the object's details and `status` (0 available, 1 unavailable). */
  object(label: string): string | undefined;
  sectionsOpen(): Uint32Array;
  resize(width: number, height: number, dpr: number, maxDpr: number): void;
  frame(time: number): boolean;
  pointer(kind: number, id: number, type: number, x: number, y: number, time: number): void;
  wheel(dx: number, dy: number, x: number, y: number, ctrl: boolean, time: number): boolean;
  takeEvents(): string | undefined;
}
