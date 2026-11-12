-- The editor that took a chart's editing lock (D-96): an id the client makes
-- for itself (the console's designer: one per browser), so that one
-- person's two browsers are two holders. NULL when the request named none.
ALTER TABLE charts ADD COLUMN lock_editor text;
