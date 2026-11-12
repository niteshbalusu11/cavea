# Charts and versions

A chart is a venue's seat map. This page explains its draft, its published versions, and how events use them.

## Draft and chart document

Every chart has one **draft**: the version being edited. The draft is a JSON **chart document**: floors, sections, blocks of rows, seats, categories, shapes, text and background images. Staff edit it in the designer, which saves as they go. You can also read and write it with the API ([get the draft](../api/charts.md#getDraft), [save the draft](../api/charts.md#saveDraft)), or export it from the designer as a file and import it into another chart or server.

A chart has a **key** you choose, such as `grand-hall`: 1 to 128 characters, unique in the workspace, and fixed once the chart exists. The API names charts by key.

Saving a draft through the API takes the draft's `draft_etag` in `If-Match`, so that two writers cannot overwrite each other unnoticed: a stale one is `409 version_conflict`. While someone edits a chart in the designer, they hold its **editing lock**; others see who is editing and open it read-only, unless they take the lock over.

## Validation

The designer checks the draft as it changes and lists findings in its Validation panel: errors, which stop publishing, such as two seats with one label, and warnings, which do not. The server runs the same checks when publishing ([validate](../api/charts.md#validateDraft) runs them without publishing).

## Publishing and versions

**Publishing** compiles the draft into a compact binary file and stores it as a new **version** of the chart: version 1, 2, and so on. A version never changes. Editing the draft afterwards changes nothing that is published until the next publish.

Each version has an `id` (`cv_…`), its number, a content hash, seat and row counts, and a URL under `/v1/public/charts/` from which seat maps download it. The file holds what a buyer may see and nothing else; any page may load it, and it can be cached forever, since its address is its content.

Publishing a draft that did not change since the last version returns that version instead of making a new one.

## Charts and events

An [event](events.md) is created on one version of a chart, the published one unless you name another, and keeps it. Publishing a new version never changes existing events: a new event uses the new version. So staff can fix a chart for future events without touching seats already sold.

## Archiving

Archiving a chart hides it from lists and stops new events on it. Its versions keep serving existing events, and its key stays taken. Archiving cannot be undone in this version.

## Labels

Every seat gets a **label** from the chart's label template, by default `{section}-{row}-{seat}`, such as `Stalls-C-12`. A seat not in a section has no section part, and the dash that would follow it goes too: `C-12`. Any part that is empty is left out with its separator, so an empty part never leaves a separator at the start or end of a label, or two side by side. Labels are unique in a version, and the API names seats by label. See [Objects and statuses](objects.md).

## In the API

[Charts](../api/charts.md): create, list and read charts; read, save and validate the draft; publish; list versions; take or release the editing lock; upload background images.
