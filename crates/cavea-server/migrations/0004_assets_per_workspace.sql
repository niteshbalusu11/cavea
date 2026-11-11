-- Assets (docs/08-server.md §6.2) belong to the workspace that uploaded them.
-- Two workspaces may upload the same bytes, so the key is (workspace, hash);
-- the public route finds a file by its hash alone, through the index.
-- No server before this migration wrote the table.
ALTER TABLE assets DROP CONSTRAINT assets_pkey;
ALTER TABLE assets ADD PRIMARY KEY (workspace_id, hash);
CREATE INDEX assets_hash ON assets (hash);
-- Images are compressed already: store them out of line, uncompressed.
ALTER TABLE assets ALTER COLUMN bytes SET STORAGE EXTERNAL;
