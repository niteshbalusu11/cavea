# 10 · AI import

**Status:** Draft v0.1 · 2026-11-10 · Priority P2 (milestone M7)

AI import turns a venue's existing seating plan (a PDF or an image) into a draft chart that a person then corrects in the designer.

## 1. Promise

- **What we promise:** most of the drawing done in about a minute, with every uncertain part clearly marked.
- **What we do not promise:** a finished chart with no review. Detection will make mistakes, and a wrong seat label reaches a printed ticket.

The feature is optional. The core product works without it and without any network access to us or to a model provider (`DECISIONS.md` D-19).

## 2. Inputs

| Input | Typical source | Difficulty |
|---|---|---|
| Vector PDF | Exported from CAD or a drawing tool | Low: seats are repeated shapes with exact coordinates |
| Raster PDF or scan | Scanned printout | Medium to high |
| PNG or JPEG | Screenshot of a website seating plan | Medium |
| Photo of a printed plan | Phone camera | High; perspective and lighting |

SVG and DXF files skip this pipeline and use the plain importer (`07-designer.md` §12).

Limits: one page or image per import, up to 50 MB, up to 12,000 px on the long side after rasterisation.

## 3. Pipeline

```
ingest ─▶ classify ─┬─▶ 1a vector extraction ─┐
                    └─▶ 1b raster detection ──┴─▶ 2 rows ─▶ 3 blocks ─▶ 4 sections
                                                    ─▶ 5 labels ─▶ 6 focal point ─▶ 7 assemble ─▶ review
```

### Stage 0 · Ingest and classify

- Open PDFs with PDFium. Count vector path objects and text objects.
- A page with thousands of small paths is **vector**; a page that is one large image is **raster**.
- For raster input: correct rotation and skew, scale to about 3,000 px on the long side.

### Stage 1a · Seat detection, vector

1. Extract every path with its transform and style.
2. Compute a **shape signature** per path: normalised bounding-box size, aspect ratio, point count, fill and stroke.
3. Cluster by signature. The seat cluster is the largest group of small, same-sized shapes (hundreds to tens of thousands of members).
4. Seat centres are the path centroids; seat size is the cluster's bounding-box size.
5. Extract text objects with positions for stage 5.

No model is involved. This stage is deterministic and should be near-perfect on clean files.

### Stage 1b · Seat detection, raster

1. Quantise colours; separate foreground marks from the background.
2. Find connected components; keep those whose size, aspect ratio and solidity are plausible for a seat.
3. Build a histogram of component sizes; the dominant peak gives the seat size.
4. Refine by template matching with the median seat patch to recover seats that touch each other or a line.
5. Score each candidate.

A learned detector is a later improvement if classical detection misses the targets in §7.

### Stage 2 · Rows

1. Build a nearest-neighbour graph over seat centres.
2. Link neighbours whose distance is close to the modal pitch.
3. Follow links into chains, keeping direction consistent.
4. Fit each chain as a line or a circular arc (robust least squares); choose the simpler fit unless the arc is clearly better.
5. Merge chains that lie on the same line or arc; a gap between them becomes an aisle.

### Stage 3 · Blocks

- Group rows that share orientation, pitch and (for arcs) centre, and that are spaced by a consistent row pitch.
- A group becomes a parametric `rowBlock`. Rows that fit no group become free `row` nodes.
- Parametric blocks matter: they are what makes the result editable instead of a cloud of points.

### Stage 4 · Sections

- Cluster blocks by proximity, separated by gaps wider than twice the row pitch.
- If the drawing has boundary shapes or section names, prefer them: a text item inside or beside a cluster names it.

### Stage 5 · Labels

| Source | Method |
|---|---|
| Vector PDF with a text layer | Associate each text item with the nearest row end (row labels) or seat (seat numbers) |
| Raster | Send crops to a vision model with the detected rows drawn and indexed; ask for the row label and the first and last seat number of each indexed row, as structured output |

Then **fit a numbering scheme** to the observations for each block: direction, start, step, odd or even, skipped letters. A scheme that explains the observations is stored as the block's rule; observations that contradict it are flagged.

Seat counts always come from detection, never from the model.

### Stage 6 · Focal point

- Look for text such as "stage", "screen", "pitch", "court", or the largest non-seat shape that the seat rows face.
- For raster input, ask the vision model for the stage location and check that most rows face it.

### Stage 7 · Assemble

- Emit a chart document (`03-chart-format.md` §2) with sections, blocks, labels and the focal point.
- Place the source as a locked reference image, aligned and scaled under the result.
- Attach a confidence value and a list of open questions to each node.
- Run the standard validation.

## 4. Review in the designer

- The draft opens with the source underneath at 50% opacity and detected seats drawn over it.
- Colour shows confidence: confident, uncertain, and unmatched marks in the source that were not turned into seats.
- A side panel lists open questions in order of impact, each with a one-click jump:
  - "Row labels in block 4 could not be read."
  - "Seat count differs from the capacity you entered: 2,148 found, 2,160 expected."
  - "Rows 3–5 of block 2 might be one curved block."
- Fix tools: merge rows, split a row, re-fit as arc or line, move rows to another section, renumber a block, add missing seats along a row.
- A capacity field lets the user state the known seat count; the difference is shown live.
- "Accept" removes the confidence overlay and turns the draft into an ordinary chart.

## 5. Use of models

| Step | Model? | Why |
|---|---|---|
| Vector seat detection | No | Exact geometry is available |
| Raster seat detection | No (classical vision), possibly later | Countable, checkable, cheap |
| Row and block fitting | No | Geometry |
| Reading labels from raster images | Yes | Small rotated text in varied fonts |
| Naming sections, locating the stage | Yes | Layout understanding |
| Explaining open questions to the user | No | Templates are enough |

Rules:

- The model is called through a provider trait. The default is the most capable current Claude vision model; the model id is configuration.
- Requests use structured outputs with a strict schema. A response that fails validation is retried once, then treated as "unknown".
- The model never returns coordinates that we use as seat positions.
- Only the venue image and crops of it are sent. No customer or buyer data exists in this flow.
- Start with the strongest model to establish quality; test cheaper models against the evaluation corpus before switching.

## 6. Deployment

- A separate stateless service, `cavea-import`, in Rust. CPU only.
- API: `POST /v1/imports` (upload) returns a job id; `GET /v1/imports/{id}` returns progress and, when done, the chart document and report.
- Two ways to run it:
  - **Hosted by us:** the designer calls our service; billed in credits. This is the default and the only paid usage-based part of the product.
  - **Self-hosted:** the customer runs the container with their own model API key.
- Uploaded files are deleted after the job completes, or after 24 hours.

## 7. Evaluation

A corpus of 40 real venue plans, collected from public sources, with ground truth drawn by hand in our own designer:

| Group | Count |
|---|---|
| Clean vector PDFs | 10 |
| Clean raster images | 15 |
| Scans and low-resolution images | 10 |
| Hard cases: photos, dense stadiums, unusual shapes | 5 |

Metrics, computed by an automated harness:

| Metric | Definition | Target, vector | Target, clean raster |
|---|---|---|---|
| Seat recall | Share of true seats with a detected seat within half a seat size | ≥ 98% | ≥ 90% |
| Seat precision | Share of detected seats that are true seats | ≥ 98% | ≥ 95% |
| Row grouping | Share of seats placed in a correctly formed row | ≥ 95% | ≥ 85% |
| Label accuracy | Share of seats whose full label is right after scheme fitting | ≥ 90% | ≥ 70% |
| Correction time | Time for a person to reach a publishable chart, against drawing from scratch | ≥ 60% saved | ≥ 60% saved |

**Ship gate:** the feature is released only when the targets are met on the clean groups. Scans and hard cases are reported but do not gate.

## 8. Risks

| Risk | Mitigation |
|---|---|
| Wrong labels reach tickets | Review is mandatory; conflicts are flagged; capacity check; validation |
| Users upload plans they do not own | Terms of service; files are not retained |
| Cost per import is unknown | Measure on the corpus before pricing credits |
| Quality on raster input disappoints | Ship vector first; raster only when it passes the gate |
| The model provider changes or is unavailable | Provider trait; the pipeline degrades to "labels unknown" rather than failing |

## 9. Phases

| Phase | Scope |
|---|---|
| I1 | Vector PDFs end to end, no model; review interface |
| I2 | Raster seat detection and row fitting |
| I3 | Label reading and section naming with a vision model |
| I4 | Hosted service, credits, polish on the review interface |
