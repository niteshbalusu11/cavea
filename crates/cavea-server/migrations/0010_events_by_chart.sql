-- A chart's events, counted for the charts list (docs/09-console.md §3.1):
-- each event names a chart version, so the count reads the chart's versions
-- (by `chart_versions (chart_id, number)`) and then their events by this.
CREATE INDEX events_chart_version ON events (chart_version_id);
