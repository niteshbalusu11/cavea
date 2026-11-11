//! Shared domain logic for every platform: the chart model, hit testing,
//! selection rules, best-available and camera maths. It performs no I/O, is not
//! async, and keeps its dependencies minimal so it compiles everywhere, from the
//! browser to the server to mobile. It depends on no other workspace crate except
//! `cavea-format`. See `docs/01-architecture.md` §2 and `docs/02-domain-model.md`.

pub mod label;
pub mod limits;
