-- Chart versions are deduplicated by the hash of the full artefact, not the
-- public one (docs/08-server.md §6.2): two drafts can compile to the same
-- public artefact and differ only in the full-only chunks (oids, ranks,
-- section priorities), and both must be publishable. The public hash, which
-- names the served file, may repeat within a chart; it is looked up across
-- all workspaces by the public chart route.

ALTER TABLE chart_versions ADD COLUMN full_hash bytea; -- 16 bytes
-- A stored full artefact carries its content hash at bytes 16 to 31 of its
-- header (docs/03-chart-format.md §4.1).
UPDATE chart_versions SET full_hash = substring(full_ccf FROM 17 FOR 16);
ALTER TABLE chart_versions ALTER COLUMN full_hash SET NOT NULL;
ALTER TABLE chart_versions DROP CONSTRAINT chart_versions_chart_id_hash_key;
ALTER TABLE chart_versions ADD CONSTRAINT chart_versions_chart_id_full_hash_key
  UNIQUE (chart_id, full_hash);
CREATE INDEX chart_versions_hash ON chart_versions (hash);
