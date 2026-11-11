-- What the console and the designer need of the server (docs/08-server.md §5).
--
-- Each event's objects per status: `counts[k]` is how many have status k, 1
-- to 15 (free is the version's object count less the rest). The commit keeps
-- it in the same statement as the rows (§4.4), so it is exactly the committed
-- `object_state`, without a scan.
ALTER TABLE events ADD COLUMN counts int4[] NOT NULL DEFAULT array_fill(0, ARRAY[15])
  CHECK (cardinality(counts) = 15);
UPDATE events e SET counts = ARRAY(
  SELECT count(o.oid)::int4
    FROM generate_series(1, 15) AS s(code)
    LEFT JOIN object_state o ON o.event_id = e.id AND o.status = s.code
   GROUP BY s.code ORDER BY s.code)
 WHERE EXISTS (SELECT FROM object_state o WHERE o.event_id = e.id);
-- The events list pages by creation, either way.
CREATE INDEX events_created ON events (workspace_id, created_at, id);

-- The designer's editing lock (docs/07-designer.md §9): who holds it (`usr_…`
-- or `key_…`), since when, and until when unless renewed; by the database's
-- clock.
ALTER TABLE charts
  ADD COLUMN lock_actor text,
  ADD COLUMN lock_since timestamptz,
  ADD COLUMN lock_expires_at timestamptz;

-- A version published with an API key names the key; `created_by` stays the
-- console user.
ALTER TABLE chart_versions ADD COLUMN created_by_key uuid;
