//! The compiled chart format (CCF) reader and writer, and the codecs for the
//! realtime wire protocol. Every input byte is treated as hostile: decoding returns
//! typed errors and never panics. It depends on no other workspace crate except
//! `cavea-core`. See `docs/03-chart-format.md` §4 and `docs/08-server.md`.
