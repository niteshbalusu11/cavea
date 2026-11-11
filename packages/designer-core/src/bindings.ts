// The shape of the module `cargo xtask build-designer` writes to ../wasm
// (wasm-bindgen output, not committed): `crates/cavea-designer/src/lib.rs`'s
// three exports. Declared here so that the package type-checks without the
// build; the boundary test checks it against the built module.

/** What the module can be instantiated from. */
export type WasmSource = Response | BufferSource | WebAssembly.Module | string | URL;

export interface Bindings {
  default(init?: { module_or_path: WasmSource | Promise<WasmSource> }): Promise<unknown>;
  Designer: new () => Handle;
}

/** `cavea_designer::Designer`: every reply is `{"ok": …}` or `{"err": Failure}`. */
export interface Handle {
  import(text: string): string;
  call(method: string, args: string): string;
  free(): void;
}
