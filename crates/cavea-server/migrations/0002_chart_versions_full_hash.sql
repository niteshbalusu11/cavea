-- Chart versions are deduplicated by the hash of the full artefact, not the
-- public one (docs/08-server.md §6.2): two drafts can compile to the same
-- public artefact and differ only in the full-only chunks (oids, ranks,
-- section priorities), and both must be publishable. The public hash, which
-- names the served file, may repeat within a chart; it is looked up across
-- all workspaces by the public chart route.

ALTER TABLE chart_versions ADD COLUMN full_hash bytea NOT NULL; -- 16 bytes
ALTER TABLE chart_versions DROP CONSTRAINT chart_versions_chart_id_hash_key;
ALTER TABLE chart_versions ADD CONSTRAINT chart_versions_chart_id_full_hash_key
  UNIQUE (chart_id, full_hash);
CREATE INDEX chart_versions_hash ON chart_versions (hash);
