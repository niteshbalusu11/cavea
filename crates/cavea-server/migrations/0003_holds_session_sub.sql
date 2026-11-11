-- Public hold tokens created with a session token are bound to the SHA-256 of
-- its `sub` (docs/08-server.md §11). The per-`sub` cap counts an event's live
-- tokens for one `sub` at every creation: an index keeps that a probe instead
-- of a scan of the event's holds.
CREATE INDEX holds_session_sub ON holds (event_id, session_sub) WHERE session_sub IS NOT NULL;
