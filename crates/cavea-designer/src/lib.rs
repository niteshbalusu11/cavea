//! The wasm-bindgen entry point for the designer (`docs/07-designer.md`
//! §2.4): one [`Designer`] holding an edit-core [`Session`], driven by JSON
//! calls named in the table of [`calls`]. `@cavea/designer-core` wraps it in
//! a typed TypeScript `Session`. Built for `wasm32-unknown-unknown` by
//! `cargo xtask build-designer`.
//!
//! Every value crosses as JSON text: a call's arguments as a JSON array, its
//! reply as `{"ok": value}` or `{"err": failure}`, where a failure is a
//! [`Failure`]. Nothing here panics on any input; a panic would be a bug in
//! the edit core, and since the module is built with `panic = "abort"` it
//! reaches JavaScript as a `WebAssembly.RuntimeError`, which the package
//! reports as its own error and treats as fatal for the module.

use cavea_edit::{CommandError, ImportError, Session};
use serde::Serialize;
use wasm_bindgen::prelude::wasm_bindgen;

pub mod calls;
#[cfg(target_arch = "wasm32")]
pub mod canvas;
pub mod paint;
pub mod scene;
#[cfg(test)]
mod tests;

/// One designer session behind the module boundary. It holds no session
/// until [`Designer::import`] opens a chart.
#[wasm_bindgen]
#[derive(Debug, Default)]
pub struct Designer {
    session: Option<Session>,
}

#[wasm_bindgen]
impl Designer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens chart document JSON (07 §12) as a new session, replacing any
    /// open one; refused with an `import` failure that keeps it.
    pub fn import(&mut self, text: &str) -> String {
        reply(Session::import(text).map(|session| {
            self.session = Some(session);
        }))
    }

    /// Runs call `method` of [`calls`] with `args`, a JSON array of its
    /// arguments, and returns `{"ok": value}` or `{"err": failure}`.
    pub fn call(&mut self, method: &str, args: &str) -> String {
        match &mut self.session {
            Some(session) => match calls::dispatch(session, method, args) {
                Ok(json) => format!("{{\"ok\":{json}}}"),
                Err(failure) => error(&failure),
            },
            None => error(&Failure::NoChart),
        }
    }
}

/// Why a call returned no value. In JSON tagged by `kind`.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Failure {
    /// The edit core refused the call; nothing changed.
    Command { error: CommandError },
    /// The chart document cannot be opened; the open session, if any, is
    /// kept.
    Import { error: ImportError },
    /// The arguments are not what the call takes (their count, a type, a
    /// number out of range, an unknown field): serde's message. Nothing
    /// changed.
    Arguments { message: String },
    /// No call has this name.
    UnknownCall { name: String },
    /// No chart is open: `import` one first.
    NoChart,
    /// A reply could not be written as JSON: a bug.
    Encode { message: String },
}

impl From<CommandError> for Failure {
    fn from(error: CommandError) -> Self {
        Self::Command { error }
    }
}

impl From<ImportError> for Failure {
    fn from(error: ImportError) -> Self {
        Self::Import { error }
    }
}

fn reply<T: Serialize, E: Into<Failure>>(result: Result<T, E>) -> String {
    match result
        .map_err(Into::into)
        .and_then(|value| calls::encode(&value))
    {
        Ok(json) => format!("{{\"ok\":{json}}}"),
        Err(failure) => error(&failure),
    }
}

fn error(failure: &Failure) -> String {
    match serde_json::to_string(failure) {
        Ok(json) => format!("{{\"err\":{json}}}"),
        // Not reached: a failure is strings and numbers.
        Err(_) => r#"{"err":{"kind":"encode","message":"failure"}}"#.to_owned(),
    }
}
