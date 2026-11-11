//! The seat engine: one single-writer actor per event holding seat state in
//! memory, the seat state machine, holds and their expiry, idempotency, and the
//! storage trait that group-commits to PostgreSQL. See `docs/08-server.md` and
//! `docs/02-domain-model.md`.
