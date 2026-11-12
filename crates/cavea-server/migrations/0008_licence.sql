-- The licence uploaded through the console (docs/12-security-licensing.md
-- §7.3, D-80): one row, the file as uploaded, so that every node reads the
-- same one. A licence given in CAVEA_LICENCE is never stored.
CREATE TABLE licence (
  id          smallint PRIMARY KEY CHECK (id = 1),
  document    bytea NOT NULL,
  uploaded_at timestamptz NOT NULL DEFAULT now(),
  uploaded_by text NOT NULL
);
