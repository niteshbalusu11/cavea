//! The wasm-bindgen entry point for the buyer-facing renderer. It glues the
//! renderer engine to the browser: DOM, input, the accessibility layer and the
//! public API that `@cavea/renderer` wraps in TypeScript. Built for
//! `wasm32-unknown-unknown` by `cargo xtask build-web`. See `docs/05-web-sdk.md`.

use wasm_bindgen::prelude::wasm_bindgen;

#[cfg(any(target_arch = "wasm32", test))]
mod events;
#[cfg(test)]
mod traces;
#[cfg(target_arch = "wasm32")]
mod view;
#[cfg(test)]
mod wire_fixture;

/// The version of this crate, so the host page can tell which build it loaded.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
