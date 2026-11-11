# 03 · Chart format

**Status:** Draft v0.1 · 2026-11-10

A chart exists in two forms.

| Form | File | Edited by | Read by | Optimised for |
|---|---|---|---|---|
| **Chart document** | JSON | Designer | Designer, compiler | Editing, diffing, migration |
| **Compiled chart (CCF)** | Binary | Nobody | Renderer, server | Load speed and size |

## 1. Coordinates and units

- One **chart unit** (cu) is nominally one centimetre. Charts need not be to scale.
- The origin is arbitrary. X grows right, Y grows down.
- Angles are degrees, clockwise.
- Defaults: seat size 44 cu, seat pitch 55 cu, row pitch 95 cu.
- Compiled positions are quantised to **0.25 cu**.

## 2. Chart document (schema 1)

Field names are exact. A field the schema does not define is an error, not ignored, so a document that parses is fully represented and the designer saves everything it loads. Optional fields with a default are written back with their value.

### 2.1 Top level

```jsonc
{
  "schema": 1,
  "name": "Royal Theatre",
  "settings": {
    "seatSize": 44,
    "seatShape": "circle",                 // circle | rounded | chair
    "labelTemplate": "{section}-{row}-{seat}",
    "tableLabelTemplate": "{section}-{table}-{seat}",
    "rowLabelDisplay": "both",             // none | start | end | both
    "alphaOverflow": "double"              // after Z: "double" → AA, BB · "spreadsheet" → AA, AB
  },
  "nextOid": 20431,                        // allocation counter; never decreases
  "categories": [
    { "key": "vip", "label": "VIP", "color": "#B45309", "accessible": false }
  ],
  "floors": [
    { "id": 1, "name": "Stalls", "level": 0, "focalPoint": [0, -800] }
  ],
  "nodes": [ /* see 2.2 */ ]
}
```

### 2.2 Nodes

Every node has these fields:

| Field | Type | Notes |
|---|---|---|
| `id` | u32 | From the `nextOid` counter. Unique in the chart |
| `type` | string | One of the types below |
| `name` | string, optional | Shown in the layers panel |
| `floor` | u32 | Floor id. Children inherit from their section |
| `origin` | `[x, y]` | Position in the parent's space |
| `rotation` | number | Degrees |
| `locked`, `hidden` | bool, optional | Editor state. Hidden nodes are not compiled |

Node types:

| Type | Bookable | Purpose |
|---|---|---|
| `section` | No | Named container with an outline |
| `rowBlock` | Seats | Parametric block of rows |
| `row` | Seats | One free-form row along a path |
| `table` | Seats or whole | Round or rectangular table with seats |
| `booth` | Yes | Single bookable rectangle |
| `area` | By quantity | General-admission zone |
| `shape` | No | Rectangle, ellipse, polygon, line, path |
| `text` | No | Label |
| `icon` | No | Entrance, exit, stairs, toilets, bar, lift |
| `image` | No | Reference (designer only) or background (shipped) |

### 2.3 `section`

```jsonc
{
  "id": 10, "type": "section", "floor": 1,
  "label": "Orchestra",
  "outline": "auto",              // or { "points": [[x, y], ...] }
  "category": "standard",         // default for children
  "priority": 1.0,                // best-available bias; lower ranks first
  "entrance": "Door 3",
  "children": [ /* rowBlock, row, table, booth, area, shape, text */ ]
}
```

With `"outline": "auto"` the compiler computes a concave hull around the children with padding of half a row pitch.

### 2.4 `rowBlock`

```jsonc
{
  "id": 11, "type": "rowBlock",
  "origin": [0, 0], "rotation": 0,
  "seatPitch": 55, "rowPitch": 95,
  "curve": { "kind": "arc", "radius": 2400 },   // none | arc | bulge
  "align": "center",                            // left | center | right
  "stagger": 0,                                 // fraction of a pitch shifted on alternate rows
  "skew": 0,                                    // degrees of horizontal shear
  "category": "standard",
  "rowLabels":  { "scheme": "alpha", "start": "A", "skip": ["I", "O"], "direction": "front-to-back" },
  "seatLabels": { "scheme": "numeric", "start": 1, "step": 1, "direction": "ltr", "continuous": false },
  "rows": [
    { "rid": 12, "seats": 30, "offset": 0, "oids": [[100, 30]] },
    { "rid": 13, "seats": 32, "offset": 0, "oids": [[130, 32]], "label": "BB", "removed": [15, 16] }
  ],
  "seatOverrides": {
    "104": { "flags": ["accessible"], "category": "access" },
    "105": { "flags": ["companion"] },
    "131": { "label": "W1", "nudge": [0, -10] }
  }
}
```

Rules:

- Row index 0 is the front row (nearest the focal point).
- `curve.kind = "arc"`: rows are concentric arcs. Row *i* has radius `radius + i × rowPitch`; seats are spaced `seatPitch` along the arc. A positive radius curves towards the front.
- `curve.kind = "bulge"`: each row is displaced by `amount × (1 − (2t − 1)²)` for `t` from 0 to 1 along the row.
- `oids` is a list of `[first, count]` runs covering the row's seat positions in physical order. Runs keep identities stable when seats are added at either end. The runs must cover exactly `seats` positions.
- `removed` lists seat positions (0-based, from the left end) that leave a physical gap. A gap splits the row into segments. A removed position keeps its `oid` and its seat number, so removing a seat never renames the others.
- A row's `label` replaces the scheme's label; the row keeps its place in the sequence.
- `seatOverrides` is keyed by `oid`. An override may set `flags` (`accessible`, `companion`, `restrictedView`, `aisle`, `removable`), `category`, `label` and `nudge` (`[dx, dy]` in node space).

### 2.5 Label schemes

Row labels:

| Field | Values |
|---|---|
| `scheme` | `alpha`, `alpha-lower`, `numeric`, `roman`, `custom` |
| `start` | First label |
| `step` | Increment, default 1 |
| `skip` | Labels to omit, for example `["I", "O"]` |
| `direction` | `front-to-back`, `back-to-front` |
| `prefix`, `suffix` | Strings |
| `custom` | Explicit list for the `custom` scheme |

Seat labels:

| Field | Values |
|---|---|
| `scheme` | `numeric`, `alpha`, `odd`, `even`, `custom` |
| `start`, `step` | As above |
| `direction` | `ltr`, `rtl`, `center-out` (1 at the centre, odd numbers one side, even the other), `serpentine` (alternate direction per row) |
| `continuous` | Numbering carries on from the previous row |
| `prefix`, `suffix` | Strings |

Tables also take `clockwise` and `counterclockwise`; rows reject them.

Details:

- Both rules accept every scheme; the tables show the usual ones. `start` is a number or a label in the scheme (`"A"`, `"XII"`, `"7"`). The defaults are the first label (`1`, `A`, `I`), and `2` for `even`.
- `odd` and `even` must start on a number of that parity; their `step` counts odd or even numbers.
- `alpha` runs `A`–`Z`, then follows `settings.alphaOverflow`. `roman` covers 1 to 3,999. A label longer than 64 bytes, prefix and suffix included, is an error.
- `skip` lists bare labels (no prefix or suffix), compared case-insensitively. With a `step` above 1 it removes values from the stepped sequence: `start "A"`, `step 2`, `skip ["I"]` gives A, C, E, G, K. A label not on the sequence has no effect, and each label must be listed: skipping `I` does not skip `II`.
- `custom` labels in order from the list; `start`, `step` and `skip` do not apply. A list shorter than the row is an error.
- Seat positions are numbered from the left end of the row in node space, removed positions included.
- `center-out` puts the first label on the centre position, then alternates one to the right, one to the left. In an even-length row the centre is the left of the two middle positions, so the left half carries the 1st, 3rd, 5th labels and the right half the 2nd, 4th, 6th: `5 3 1 2 4 6`.
- `serpentine` numbers the 1st, 3rd, 5th rows in row-label order left to right and the others right to left.
- "Previous row" for `continuous` and the row order for `serpentine` follow the row labels: with `back-to-front` numbering starts at the back.

### 2.6 Other bookable nodes

```jsonc
{ "id": 40, "type": "row", "path": { "kind": "polyline", "points": [[0,0],[500,40]] },
  "seatPitch": 55, "seats": 9, "oids": [[400, 9]], "label": "A", "seatLabels": { ... },
  "category": "standard", "removed": [], "seatOverrides": {} }   // last three optional, as in rowBlock

{ "id": 50, "type": "table", "shape": "round", "radius": 90, "seats": 8,
  "bookAs": "seats",                       // seats | table
  "label": "T1", "oids": [[500, 8]], "tableOid": 508,
  "seatLabels": { "scheme": "numeric", "start": 1, "direction": "clockwise" } }

{ "id": 60, "type": "booth", "size": [200, 150], "label": "Box 1", "oid": 600, "category": "vip" }

{ "id": 70, "type": "area", "shape": { "points": [[0,0],[900,0],[900,600],[0,600]] },
  "label": "Standing", "capacity": 500, "oid": 700, "category": "ga" }
```

`path.kind` is `polyline` only: the designer stores a curved row as points along the curve. The node `id` is also the row's id. A `table` with `"shape": "rect"` takes `size: [w, h]` instead of `radius`.

### 2.7 Decoration nodes

```jsonc
{ "id": 80, "type": "shape", "kind": "rect", "size": [1200, 300], "cornerRadius": 20,
  "style": { "fill": "#1F2937", "stroke": null, "strokeWidth": 0, "opacity": 1 }, "layer": "below" }

{ "id": 81, "type": "text", "text": "STAGE", "size": 120, "weight": 600,
  "color": "#F9FAFB", "align": "center", "minZoom": 0 }

{ "id": 82, "type": "icon", "icon": "entrance", "size": 80 }

{ "id": 83, "type": "image", "asset": "b3a1…", "size": [4000, 3000], "opacity": 0.5,
  "role": "reference" }                    // reference | background
```

`shape` geometry by `kind`: `rect` and `ellipse` take `size` (and `rect` an optional `cornerRadius`); `polygon` (closed) and `line` (open) take `points`; `path` takes `d`, SVG path data.

### 2.8 Migrations

- `schema` is an integer. `cavea-compiler` holds a migration function for each step.
- The server stores the document as uploaded and migrates on read. The designer saves in the current schema.
- A migration never changes an `oid`.

## 3. Compiler

### 3.1 Pipeline

1. **Parse and migrate** to the current schema.
2. **Validate structure:** references, limits, required fields.
3. **Expand** parametric nodes into seats: apply curve, skew, stagger, alignment, removals and overrides.
4. **Transform** everything to world coordinates.
5. **Segment rows:** break at removed seats and wherever the gap between neighbours exceeds 1.5 × the seat pitch.
6. **Label:** apply schemes, then overrides, then the chart-level template.
7. **Check uniqueness** of labels.
8. **Rank seats** by distance and angle to the focal point (§3.3).
9. **Order and index:** assign `idx` (see `02-domain-model.md` §3).
10. **Chunk:** split sections larger than 4,096 seats into spatial chunks of whole rows.
11. **Outline:** compute section hulls, label anchors (pole of inaccessibility) and the largest font size that fits.
12. **Tessellate** shapes, outlines and table shapes into triangles with `lyon`.
13. **Quantise** positions.
14. **Encode** chunks, compute the content hash (BLAKE3, truncated to 128 bits).
15. **Emit** two artefacts: the public CCF for renderers and the full CCF for the server, plus statistics.

The compiler is deterministic: the same document always produces the same bytes.

### 3.2 Validation

Errors block publishing. Warnings do not, unless the workspace promotes them.

| Code | Severity | Condition |
|---|---|---|
| E001 | Error | Duplicate object label |
| E002 | Error | Object without a category |
| E003 | Error | Row or seat with an empty label |
| E004 | Error | Area without a capacity |
| E005 | Error | Invalid geometry: self-intersecting outline, zero size, non-finite number |
| E006 | Error | A limit in `02-domain-model.md` §14 exceeded |
| E007 | Error | Reference to an unknown category, floor or asset |
| E008 | Error | Duplicate category key |
| E009 | Error | Duplicate or reused `oid` |
| E010 | Error | Unsupported schema or failed migration |
| W101 | Warning | Overlapping seats: centres closer than 0.6 × seat size |
| W102 | Warning | Floor without a focal point (best-available falls back to row order) |
| W103 | Warning | Row with a single seat |
| W104 | Warning | Seat outside its section's explicit outline |
| W105 | Warning | Unused category |
| W106 | Warning | No seat flagged accessible |
| W107 | Warning | Two category colours too similar, or low contrast against the background |
| W108 | Warning | Text too small to read at fit zoom |
| W109 | Warning | Section without a label |
| W110 | Warning | Chart extent above 200,000 cu (precision) |

Each finding carries the node id, so the designer can select the offender.

### 3.3 Seat rank

Rank drives best-available. Lower is better. Stored as u16 in the full CCF only.

For a seat at position `p` on a floor with focal point `F` and forward axis `a` (the unit vector from `F` to the centroid of the floor's seats):

```
d      = |p − F|
cosθ   = ((p − F) / d) · a
raw    = d × (1 + 0.6 × (1 − cosθ)) × section.priority
raw   += floor_order × D          // D = the largest raw value on any floor; lower levels first
rank   = dense rank of raw, scaled to 0…65535
```

The constant 0.6 and the formula are versioned in the CCF (`rank_algo = 1`) so they can change without breaking old charts.

## 4. Compiled Chart Format (CCF v1)

### 4.1 Container

All integers are little-endian. Every chunk payload starts on a 4-byte boundary.

```
Header (32 bytes)
  0   magic          4 bytes   "CCF1"
  4   version        u16       1
  6   flags          u16       bit 0: full (server) artefact
  8   chunk_count    u32
  12  total_len      u32
  16  content_hash   16 bytes  BLAKE3-128 of everything after the header

Chunk directory (chunk_count × 16 bytes)
  tag        4 bytes
  encoding   u8
  reserved   3 bytes   zero
  offset     u32       from the start of the file
  length     u32       bytes
  (the item count is the first varint inside the payload)

Chunk payloads
```

Encodings:

| Id | Name | Layout |
|---|---|---|
| 0 | Raw | Fixed-width little-endian records |
| 1 | Varint | LEB128 unsigned integers |
| 2 | Delta | Zigzag LEB128 of differences from the previous value |
| 3 | Run-length | Pairs of (value, run length), both varints |
| 4 | Strings | Count, then u32 offsets, then UTF-8 bytes |

### 4.2 Chunks

| Tag | Content | Encoding | Artefact |
|---|---|---|---|
| `META` | Name, bounds, default seat size and shape, label templates, counts, `rank_algo`, compiler version | Raw | Both |
| `STRS` | String table; other chunks refer to strings by index | Strings | Both |
| `CATS` | Per category: key, label, colour (RGBA8), flags | Raw | Both |
| `FLRS` | Per floor: name, level, bounds, focal point, `idx` range | Raw | Both |
| `SECT` | Per section: label, floor, bounding box, label anchor and maximum font size, outline mesh range, seat `idx` range, row range, default category, entrance | Raw | Both |
| `CHNK` | Per render chunk: section, seat `idx` range, bounding box | Raw | Both |
| `ROWS` | Per row: section, label, first seat `idx`, seat count, seat size, shape, label-rule index, bounding box, label anchors | Raw | Both |
| `RSEG` | Per row segment: row, first seat offset, count | Raw | Both |
| `LBLR` | Label rules: scheme, start, step, direction, prefix, suffix | Raw | Both |
| `LBLX` | Label exceptions: (`idx`, string index) | Varint | Both |
| `SPOS` | Seat positions. Per row: the first seat's quantised (x, y), then deltas | Delta | Both |
| `SCAT` | Seat categories | Run-length | Both |
| `SFLG` | Seat flags | Run-length | Both |
| `OBJS` | Non-seat objects: kind, label, category, capacity, bounding box, label anchor, mesh range | Raw | Both |
| `TBLS` | Table decorations: shape, centre, size, rotation, seat `idx` range | Raw | Both |
| `MESH` | Vertices (x, y as f32; RGBA8 colour), u32 indices, draw ranges by layer | Raw | Both |
| `TEXT` | Text objects: string, position, size, angle, colour, alignment, weight, minimum zoom | Raw | Both |
| `ICON` | Icons: key, position, size | Raw | Both |
| `IMGS` | Background images: asset hash, bounding box, opacity | Raw | Both |
| `SOID` | Seat and object `oid`s | Delta | Full only |
| `SRNK` | Seat ranks | Delta | Full only |
| `OPRI` | Section priorities for best-available | Raw | Full only |

Seat flags (`SFLG`, one byte):

| Bit | Meaning |
|---|---|
| 0 | Accessible |
| 1 | Companion |
| 2 | Restricted view |
| 3 | Aisle seat |
| 4 | Removable |
| 5–7 | Reserved, zero |

### 4.3 Why these encodings

- A straight row has a constant step between seats, so `SPOS` deltas repeat and compress to almost nothing under brotli.
- Categories and flags are constant over long runs, so run-length encoding makes them tiny before transport compression.
- Seat labels are not stored. The renderer generates them from the row's rule, with `LBLX` for the exceptions.
- Seat angles are not stored. For oriented seat shapes the renderer derives the angle from each seat's neighbours.

### 4.4 Reader requirements

- Treat the file as hostile: validate every offset, length and count against the file size before use.
- Reject overlapping chunks, unknown encodings and counts that exceed the limits in `02-domain-model.md` §14.
- Skip unknown chunk tags. Fail on a missing required chunk.
- Allocate once per array, sized from validated counts.
- Decode is a single forward pass. Targets: ≤ 5 ms per 100,000 seats on a desktop, ≤ 20 ms on the reference Android phone.
- The decoder has a fuzz target and must never panic (`13-testing-benchmarks.md` §5).

### 4.5 Size

Targets for the public artefact, to be measured with the synthetic corpus:

| Chart | Raw | Brotli |
|---|---|---|
| Theatre, 2,000 seats | ≤ 40 KB | ≤ 10 KB |
| Arena, 20,000 seats | ≤ 250 KB | ≤ 60 KB |
| Stadium, 100,000 seats | ≤ 1 MB | ≤ 300 KB |

### 4.6 Delivery

- URL: `/v1/public/charts/{chart_key}/versions/{hash}.ccf`.
- Headers: `Cache-Control: public, max-age=31536000, immutable`; `Content-Type: application/vnd.cavea.ccf`; CORS open.
- Precompressed at publish time with brotli (quality 11) and gzip; the server picks by `Accept-Encoding`.
- Background images are separate, also content-addressed, and load after the first frame.
- Private charts (signed URLs) are P2.

## 5. Thumbnails

- On publish, the designer renders a 640 × 480 PNG with the renderer and uploads it with the version.
- The console and the customer's back office use it in lists.

## 6. Import and export

| Direction | Format | Priority |
|---|---|---|
| Export | Chart document JSON | P0 |
| Import | Chart document JSON | P0 |
| Import | CSV: `section,row,seat,x,y,category` | P1 |
| Import | seats.io chart JSON (from their API) | P1 |
| Import | SVG, DXF | P2 |
| Import | PDF or image through AI import | P2 |
| Export | SVG (for print) | P2 |

## 7. Synthetic charts

`cavea gen` produces charts for tests and benchmarks.

| Preset | Seats | Shape |
|---|---|---|
| `theatre` | 2,000 | Three sections, curved rows, one balcony |
| `arena` | 20,000 | Bowl of 40 sections around a rectangular floor |
| `stadium-60k` | 60,000 | Two tiers, 120 sections |
| `stadium-100k` | 100,000 | Three tiers |
| `stress-250k`, `stress-500k` | 250,000, 500,000 | Uniform tiers, for limits |
| `banquet` | 1,200 | 150 round tables of 8 |
| `festival` | 50 objects | Areas and booths only |

Presets are seeded and deterministic, so benchmark runs are comparable.
