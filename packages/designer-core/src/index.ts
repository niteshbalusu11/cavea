// `@cavea/designer-core`: the designer's edit core (`cavea-edit`) in
// WebAssembly, as a typed `Session` (docs/07-designer.md §2.4). Framework
// free and DOM free: the shell renders the views and sends the calls. The
// WASM under ../wasm is built by `cargo xtask build-designer` and is not
// checked in; the types in ./generated are generated from the Rust
// definitions by `cargo xtask types`.

export { DesignerError, loadDesigner, Session } from "./session.ts";
export type { Designer, DesignerFailure, LoadOptions } from "./session.ts";
export type { WasmSource } from "./bindings.ts";
export type * from "./generated/calls.ts";
