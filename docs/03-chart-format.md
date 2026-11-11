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
| `floor` | u32, optional | Floor id. Children inherit from their section and may not name another floor. A node outside a section without one is on the first floor listed; a document that lists no floors has one implicit floor (empty name, level 0, no focal point) |
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

**Explicit outline.** `{ "points": [...] }` lists the polygon's corners in order, in the section's own space (before its `origin` and `rotation`). Repeated consecutive points, and a last point equal to the first, are dropped. The rest must be at least three points not all on one line (else E005, zero size), at most 4,096 (E006), and no two edges may meet except neighbouring edges at their shared point (else E005, self-intersecting).

**Automatic outline.** With `"outline": "auto"` the compiler computes a concave hull around the section's seats with padding of half a row pitch; decoration children do not count. The padding `p` is half the largest `rowPitch` of the section's row blocks, or 47.5 (half the default) without one, and at least 1. In the section's own space, with `n` seat centres whose bounding box is `W × H`:

1. **Grid.** Square cells of side `h = max(p / 3, (W + H) / (8√n + 32))`: a third of the padding, coarser in a sparse section so that the grid stays within a few dozen cells per seat. The closing radius is `k = ⌈2p / h⌉` cells and the drawing radius `r = p + 2.0625h + kh`. The grid reaches `⌈r / h⌉ + 1` cells beyond the box on every side.
2. **Draw.** A cell is filled when its centre is within `r` of a seat centre.
3. **Close.** A filled cell is emptied when the centre of an empty cell, or of a cell outside the grid, is within `kh` of its own. Drawing at `r` and eroding by `kh` is a closing by about two row pitches: aisles, removed seats and the space between blocks fill in, and every cell within `p + 2.0625h` of a seat stays filled.
4. **Fill holes.** Empty cells that are not 4-connected to the grid's edge are filled.
5. **Trace.** When the filled cells form one 8-connected region, its outer boundary is walked along cell edges with the region on the right, from the top edge of the first filled cell in row-major order. At each corner the walk turns left if the cell ahead on the left is filled, else goes straight if the cell ahead on the right is filled, else turns right. The ring is the midpoints of the walked edges.
6. **Simplify.** Douglas–Peucker over the ring with tolerance `h`, measuring distance to the segment, anchored at the ring's first point and the point farthest from it (the earlier one on a tie), keeping on each span its farthest point if it is more than `h` away (the earlier one on a tie), and stopped once the spans scanned hold more than 64 points per ring point plus 1,024.
7. **Fallback.** If the region is not one 8-connected component, the walk does not close within four steps per cell, the simplification is stopped, or its result has fewer than 3 or more than 4,096 points or is not simple, the outline is the convex hull of the midpoints of every edge between a filled and an empty cell.

Steps 5–7 work in whole half-cells, which `f64` holds exactly; the result is mapped back to chart units. The outline is at least `p` from every seat centre: a traced edge is within 0.71h of an emptied cell's centre, which is more than `p + 2.0625h` from every seat, the midpoints are within 0.36h of the edges, and simplification moves the line by at most `h`. It stays within about `r + h` of the seats. A section without seats has no automatic outline.

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
- Geometry is in the node's own space, before `origin` and `rotation`. Row *i*'s baseline is `y = i × rowPitch`, so rows run towards +y. A row of `seats` positions has length `L = (seats − 1) × seatPitch`; position *k* lies at distance `s = a + offset + k × seatPitch` along the row, where `align` sets `a`: `left` 0, `center` −L/2, `right` −L. So the alignment line is `x = 0`. `offset` is in chart units.
- `stagger` shifts odd rows (1, 3, …) by `stagger × seatPitch` along the row.
- `curve.kind = "arc"`: rows are concentric arcs. Row *i* has radius `radius + i × rowPitch`; seats are spaced `seatPitch` along the arc. A positive radius curves towards the front. Exactly: with `R = radius + i × rowPitch`, the seat at distance `s` is at `(R sin(s/R), R cos(s/R) − radius)`, so the centre is `(0, −radius)`. A negative radius curves away from the front. It is an error for `R` to reach zero or change sign, or for a seat to be more than half a turn from the apex.
- `curve.kind = "bulge"`: each row is displaced by `amount × (1 − (2t − 1)²)` for `t` from 0 to 1 along the row. The displacement is along +y, so a positive bulge curves the same way as a positive arc; `t = k / (seats − 1)`, and 0.5 for a single seat.
- `skew` shears the block: `x' = x − y × tan(skew)`, so the block's sides turn clockwise. `|skew|` is below 90.
- `oids` is a list of `[first, count]` runs covering the row's seat positions in physical order. Runs keep identities stable when seats are added at either end. The runs must cover exactly `seats` positions.
- `removed` lists seat positions (0-based, from the left end) that leave a physical gap. A gap splits the row into segments. A removed position keeps its `oid` and its seat number, so removing a seat never renames the others.
- A row's `label` replaces the scheme's label; the row keeps its place in the sequence.
- `seatOverrides` is keyed by `oid`. An override may set `flags` (`accessible`, `companion`, `restrictedView`, `aisle`, `removable`), `category`, `label` and `nudge` (`[dx, dy]` in node space, added after curve and skew).
- At most 4,095 positions per row (`02-domain-model.md` §14). Expansion uses only correctly rounded arithmetic and its own trigonometry, so positions are bit-identical on every platform and in WASM.

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

A `row` places position *k* at `k × seatPitch` from its first point, measured along the path; positions past the end continue along the last segment. `path.kind` is `polyline` only: the designer stores a curved row as points along the curve. The node `id` is also the row's id. A `table` with `"shape": "rect"` takes `size: [w, h]` instead of `radius`.

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

#### 3.1.1 Pipeline rules

- **Schema and structure (1–2).** Only the current schema compiles (E010). Hidden nodes and their children are not compiled. `table`, `booth` and `area` nodes, and a section inside a section, are not compiled yet: each is reported as `Unsupported` and blocks the compile, so a bookable object is never left out. Decoration nodes are skipped until step 12 handles them.
- **Transform (4).** A node's points are rotated about its own origin by `rotation`, then moved by `origin`, into its parent's space; a section does the same into world space. Rotation is first reduced exactly to whole turns (the remainder of `rotation` divided by 360, as C's `fmod`), then uses the compiler's own sine and cosine, like expansion.
- **Segments (5).** Within a row, a segment continues from one seat to the next while their positions are consecutive (no removed position between) and the distance between them, in node space after nudges, is at most 1.5 × the seat pitch; a larger gap starts a new segment. A row whose positions are all removed, or that has none, holds no seats and is not compiled; its seat rule is not stored.
- **Labels (6–7).** A seat's label is its override or its row's rule. The full label is the chart's `labelTemplate` with `{section}`, `{row}` and `{seat}` replaced in one pass by the section label, the row label and the seat label; any other text, braces included, is copied as is. `{section}` is empty for a row outside any section or in a section without a label. Full labels of compiled seats (removed positions are not objects) are unique in the chart (E001) and at most 64 bytes (E006).
- **Compiled labels.** Each row stores its label whole and the index of its seat rule; each distinct rule is stored once in `LBLR`, with its custom list only for the `custom` scheme and an empty prefix or suffix as `NONE`. `serpentine` resolves to `ltr` on rows whose numbering order is even and `rtl` on odd, and `continuous` to `ordinal_base`. Every seat with a label override is listed in `LBLX`, whether or not its text matches the rule.
- **`idx` order (9).** Floors in the order the document lists them. On each floor, its sections in document order, then the rows outside any section in document order. Within a section, its children in document order; a block's rows front to back; a row's seats left to right, removed positions skipped. `SECT` lists sections in this order, `FLRS` floors in document order, and a row outside any section has section `NONE`.
- **Chunks (10).** Whole rows are packed into render chunks in `idx` order: a new chunk starts when the section changes (for rows outside sections, the floor) or when the next row would take the chunk past 4,096 seats. A row has at most 4,095 seats, so it always fits.
- **Outline (11–12).** A section's outline (§2.3) is placed in the world by the section's rotation and origin; an outline vertex outside the quantised range is E005. Its triangles (fill, non-zero rule, by `lyon`) are the section's outline range in `MESH`, every vertex white (§4.2.3). The section's bounds cover its seats and its outline, outline vertices rounded outward to quarter units.
- **Label anchor (11).** The pole of inaccessibility of the world outline, quantised: Mapbox's polylabel search over square cells, starting with cells of side `max(min(w, h), max(w, h) / 256)` covering the outline's `w × h` bounding box and with the centroid and the box centre as candidates, taking cells best potential first (the earlier cell on a tie), to a precision of `max(0.5, (w + h) / 512)` cu; no cell is split once 1,024 cells have been made.
- **Label size (11).** The largest size `s`, in quarter units from 0 to 65,535, at which the section's label fits the outline under this text model: the label is a box centred on the quantised anchor, axis-aligned in world space, `0.6 × c × s` wide and `1.2 × s` tall for a label of `c` Unicode scalar values, and it fits when the anchor is inside the outline and no outline edge touches the box. A section without a label has size 0. A renderer draws the label at most this size, centred on the anchor, and may assume the same model.
- **No outline.** A section without seats and with an automatic outline has none: an empty outline range, anchor (0, 0), label size 0 and bounds (0, 0, 0, 0).
- **Quantise (13).** A coordinate *x* in chart units becomes the integer nearest 4*x*, ties to even. A position with |4*x*| above 2³¹ − 1 (about ±536,870,911 cu) is an error (E005). Bounds are taken over the quantised seat positions (a section's also over its outline); an empty floor or chart, and a section with neither seats nor outline, has the box (0, 0, 0, 0). A row's label anchors lie one step beyond its end seats: `2a − b` for end seat `a` and its neighbour `b` (the seat itself in a one-seat row).
- **Emit (14–15).** `META` names the compiler and its version. The content hash of each artefact is the first 16 bytes of BLAKE3 over the file after its 32-byte header.

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

Each finding carries the node id, so the designer can select the offender. Findings about settings, categories and floors carry no node id.

- E005 also covers an explicit outline that is flat or crosses itself (§2.3), and any other invalid value: a non-finite origin or rotation, a colour that is not `#RRGGBB`, a section priority that is not finite and positive, a seat size outside 0.25 to 16,383.75 cu, and a world position outside the quantised range (§3.1.1).
- E007 also covers a floor id listed twice and a section child that names a floor other than its section's.
- E009 covers `id`, `rid` and `oid` together, since one counter allocates them, over the nodes that are compiled, and `oids` runs that do not cover their row.
- A finding the pipeline meets once per seat (no category, a full label too long, a position out of range) is reported once per node. A compile lists at most 1,000 findings.

### 3.3 Seat rank

Rank drives best-available. Lower is better. Stored as u16 in the full CCF only.

For a seat at position `p` on a floor with focal point `F` and forward axis `a` (the unit vector from `F` to the centroid of the floor's seats; +y if the centroid is `F`):

```
d      = |p − F|
cosθ   = ((p − F) / d) · a
raw    = d × (1 + 0.6 × (1 − cosθ)) × section.priority
raw   += floor_order × D          // D = the largest raw value on any floor; lower levels first
rank   = dense rank of raw, scaled to 0…65535
```

The constant 0.6 and the formula are versioned in the CCF (`rank_algo = 1`) so they can change without breaking old charts.

Rules for `rank_algo = 1`:

- Positions and focal points are the quantised ones, so the rank follows from the compiled chart. A seat at `F` has `cosθ = 1`.
- `floor_order` is the dense rank of the floor's `level` (floors on one level share it). Adding `floor_order × D` is done exactly, as an ordering by the pair (`floor_order`, `raw`), so every seat on a lower level ranks before every seat on a higher one.
- A floor without a focal point (W102) ranks by row order: `raw` is the row's place in `idx` order, so the seats of a row tie and earlier rows rank first. Section priority does not apply there.
- The dense rank is over the whole chart: keys that are equal get the same rank *r*, from 0 to *K* − 1 for *K* distinct keys. The stored rank is ⌊*r* × 65,535 / (*K* − 1)⌋, or 0 when *K* = 1. Keys are compared exactly, so the result needs no tie-break and is the same on every platform.

## 4. Compiled Chart Format (CCF v1)

### 4.1 Container

All integers are little-endian. Every chunk payload starts on a 4-byte boundary.

```
Header (32 bytes)
  0   magic          4 bytes   "CCF1"
  4   version        u16       1
  6   flags          u16       bit 0: full (server) artefact
  8   chunk_count    u32
  12  total_len      u32       the file length
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
| 0 | Raw | Count, then fixed-width little-endian records |
| 1 | Varint | Count, then LEB128 unsigned integers |
| 2 | Delta | Count, then zigzag LEB128 of differences from the previous value |
| 3 | Run-length | Count, then pairs of (value, run length), both varints |
| 4 | Strings | Count, then u32 offsets, then UTF-8 bytes |

Container rules:

- **Version.** A reader of version *n* reads versions 1 to *n* and rejects any other value, including higher ones. Additive changes (a new chunk, which old readers skip) keep the version; anything an old reader would misread raises it.
- **Reserved bits and bytes.** Header flag bits 1–15, the directory's reserved bytes, and reserved bits inside records (category flags 1–7, seat flags 5–7) are written as zero: the container writer zeroes the first two, and the compiler, which fills the records, zeroes the rest (the `cavea-format` writer carries record values as given, so a chart round-trips exactly). Readers ignore them, so a later minor revision may give them a meaning that old readers can safely ignore.
- **Payload placement.** Payloads come after the directory, in directory order, without overlap: each payload's offset is a multiple of 4, at least the previous payload's end (or the directory's end, for the first), and its end is within the file. Bytes between payloads are zero padding and are not read. `total_len` equals the file length exactly.
- **Varints** are LEB128 values of at most 5 bytes that fit in a u32. **Zigzag** maps a signed difference *d* to `(d << 1) ^ (d >> 31)`. Differences are taken modulo 2³², so every i32 or u32 sequence round-trips. The first value's difference is from 0.
- **Counts.** Every payload starts with the item count as a varint. A `Raw` payload is then exactly *count* records; records are packed, with no padding and no alignment, and a reader decodes them field by field. Nothing may follow the last item.
- **Strings** (`STRS`): the count *n*, then *n* u32 end offsets, then the UTF-8 bytes of all strings concatenated. String *i* spans bytes `[end[i−1], end[i])`, with `end[−1] = 0`. Offsets do not decrease, each falls on a character boundary, and the last equals the byte length.
- **Required chunks.** Every chunk in §4.2 for an artefact is required, empty or not: an empty table is written with count 0. A chart with no seats still has `SPOS`, `SCAT` and `SFLG`, each with count 0. A reader asked for the full artefact also requires `SOID`, `SRNK` and `OPRI` and flag bit 0. A reader that wants public data skips the full-only chunks, so it reads either artefact.
- **Determinism.** The writer emits the chunks in the order of §4.2 and the same chart always gives the same bytes.

**Content hash.** `content_hash` is the first 16 bytes of the BLAKE3 hash of `file[32..total_len]`. The `cavea-format` writer leaves the field zero and offers a function to patch it in; the artefact emitter (`cavea-compiler`) computes the hash and patches it, since the 32-byte header itself is not hashed. Readers in `cavea-format` expose the stored value and do not verify it: the browser fetches by a content-addressed URL, and the hashing code would add to the renderer's payload. The server may verify it when it stores an artefact.

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
| `LBLR` | Label rules: scheme, start, step, prefix, suffix, custom labels | Raw | Both |
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

#### 4.2.1 Value types

| Type | Bytes | Meaning |
|---|---|---|
| `str` | 4 | u32 index into `STRS` |
| `opt` | 4 | u32 index, or `0xFFFFFFFF` for none (`NONE`). `opt str` is an optional string |
| `q` | 4 | i32 position in quarter chart units (0.25 cu) |
| `point` | 8 | `q` x, `q` y |
| `box` | 16 | `q` min x, min y, max x, max y |
| `size` | 2 | u16 length in quarter chart units |
| `angle` | 2 | u16 binary angle, 65,536 per turn, clockwise |
| `rgba` | 4 | Bytes R, G, B, A |
| `range` | 8 | u32 first, u32 count |
| `opt point` | 9 | u8 presence (0 or 1), then `point`, written as zeros when absent |
| `f32` | 4 | IEEE 754, finite |

Enumerations are one byte; any other value is an error.

| Enumeration | Values |
|---|---|
| seat shape | 0 circle, 1 rounded, 2 chair |
| row label display | 0 none, 1 start, 2 end, 3 both |
| scheme | 0 numeric, 1 odd, 2 even, 3 alpha, 4 alpha-lower, 5 roman, 6 custom |
| alpha overflow | 0 double, 1 spreadsheet |
| seat order | 0 ltr, 1 rtl, 2 center-out |
| object kind | 0 table (booked whole), 1 booth, 2 area |
| table shape | 0 round, 1 rect |
| text align | 0 left, 1 center, 2 right |
| mesh layer | 0 below seats, 1 above seats |

#### 4.2.2 Record layouts

Fields are listed in byte order. Record sizes are in brackets.

- **`META`** (exactly one record) [41]: name `str`, label template `str`, table label template `str`, compiler name and version `str`, seat count u32, bounds `box`, default seat size `size`, default seat shape, row label display, `rank_algo` u8.
- **`CATS`** [13]: key `str`, label `str`, colour `rgba`, flags u8 (bit 0 accessible).
- **`FLRS`** [41]: name `str`, level i32, seats `range`, bounds `box`, focal point `opt point`.
- **`SECT`** [66]: label `opt str`, floor u32, seats `range`, rows `range`, default category `opt`, entrance `opt str`, outline `range` of `MESH` indices, bounds `box`, label anchor `point`, maximum label size `size`.
- **`CHNK`** [28]: section `opt`, seats `range`, bounds `box`.
- **`ROWS`** [60]: section `opt`, label `str`, first seat u32, seat count u16, positions u16, label rule `opt`, ordinal base u32, seat order, seat shape, seat size `size`, bounds `box`, label anchor at the start `point`, at the end `point`.
- **`RSEG`** [12]: row u32, first seat u32, seat count u16, position u16.
- **`LBLR`** [26]: scheme, alpha overflow, start u32, step u32, prefix `opt str`, suffix `opt str`, custom labels: `range` of strings.
- **`LBLX`** (Varint): count, then per exception `idx` and `str`, two varints.
- **`SPOS`** (Delta): count (the seat count), then per seat x and y as zigzag varints. Rows are taken in order; each row's first seat is a difference from (0, 0), that is absolute, and each later seat a difference from the seat before it.
- **`SCAT`**, **`SFLG`** (Run-length): count (the seat count), then (value, run) pairs. Runs are at least 1 and add up to the count. `SCAT` values index `CATS`; `SFLG` values fit in a byte.
- **`OBJS`** [53]: object kind, label `str`, category u32, capacity u32 (areas; 1 otherwise), floor u32, section `opt`, mesh `range` of `MESH` indices, bounds `box`, label anchor `point`.
- **`TBLS`** [29]: table shape, rotation `angle`, chairs u16, first seat `opt` (booked by seat: the first of *chairs* consecutive seats; booked whole: `NONE`), object `opt` (booked whole: index into `OBJS`; booked by seat: `NONE`), centre `point`, width and height as two `q`. Exactly one of first seat and object is `NONE`.
- **`MESH`** (Raw): vertex count, index count and draw count as three varints, then the vertices [12: x f32, y f32 in chart units, `rgba`], the indices [4: u32], and the draw ranges [9: mesh layer, `range` of indices].
- **`TEXT`** [27]: text `str`, position `point`, size `size`, angle `angle`, colour `rgba`, text align, weight u16, minimum zoom `f32`.
- **`ICON`** [16]: icon key `str` (`entrance`, `exit`, `stairs`, `toilets`, `bar`, `lift`), position `point`, size `size`, angle `angle`.
- **`IMGS`** [23]: asset hash `str`, bounds `box`, angle `angle`, opacity u8 (255 opaque).
- **`SOID`** (Delta): one u32 `oid` per object, in `idx` order.
- **`SRNK`** (Delta): one u16 rank per object, in `idx` order; values above 65,535 are an error.
- **`OPRI`** (Raw) [4]: one `f32` priority per section.

#### 4.2.3 Structure

- **Objects.** Seats come first: seat *i* has `idx` *i*. `OBJS` entry *j* has `idx` = seat count + *j*. `SOID` and `SRNK` cover every object.
- **Rows** partition the seats: row *r*'s first seat is the sum of the earlier rows' seat counts. Seats within a row are in physical order. A table booked by seat contributes a row of its own, so that every seat has a row, a label rule and a position. *positions* counts the row's seat positions including removed ones (at most 4,095).
- **Segments** split each row's seats, in `idx` order, into runs of at least one seat. A segment's *position* is its first seat's position in the row, counting removed positions; positions increase and stay below the row's *positions*. A row without seats has no segments.
- **Floors** and **render chunks** each partition the seats in `idx` order. A chunk holds at most 4,096 seats.
- **Seat labels** are generated, not stored. A seat at position *p* in a row has ordinal `ordinal_base + seat_ordinal(order, positions, p)` (`cavea-core::label`), and the row's label rule turns that into text. The compiler resolves `serpentine` into `ltr` or `rtl` per row and `continuous` into `ordinal_base`, so neither is stored. `ordinal_base + positions` fits in a u32, and with a `custom` rule it is at most the number of custom labels, so every position has one. A seat listed in `LBLX` uses that string instead, and a row whose rule is `NONE` labels only its listed seats. Row labels, object labels, `LBLX` labels and rule prefixes and suffixes are at most 64 bytes (`02-domain-model.md` §14); a generated label longer than that is an error when it is generated. `LBLX` is sorted by `idx`, at most one entry per seat. Row labels are stored whole, as strings.
- **Full labels.** An object's full label is `META`'s label template filled with its section's label (empty for a row with section `NONE` or a section without a label), its row's label and its seat label, by the rule in §3.1.1. Every artefact carries all four, so a consumer holding only a decoded chart can produce every label, for example to resolve an API label to an `idx`.
- **Ranges into `MESH`** (section outlines, object meshes, draw ranges) are ranges of indices. Every index is below the vertex count. The index buffer starts with the section outlines in `SECT` order, each range starting where the previous one ends (a section without an outline has an empty range there); the draw ranges follow. Decoration is not compiled yet, so the draw list is empty.
- **Mesh colour.** Section outline vertices are white, (255, 255, 255, 255): the overview tint depends on availability and the theme at run time (`04-renderer.md` §10, §19), so the renderer draws a section's outline range multiplied by that section's tint. Draw ranges carry their final colours and are drawn with a white tint.

### 4.3 Why these encodings

- A straight row has a constant step between seats, so `SPOS` deltas repeat and compress to almost nothing under brotli.
- Categories and flags are constant over long runs, so run-length encoding makes them tiny before transport compression.
- Seat labels are not stored. The renderer generates them from the row's rule, with `LBLX` for the exceptions.
- Seat angles are not stored. For oriented seat shapes the renderer derives the angle from each seat's neighbours.

### 4.4 Reader requirements

- Treat the file as hostile: validate every offset, length and count against the file size before use.
- Reject overlapping or out-of-order chunks, unknown encodings and counts that exceed the limits in `02-domain-model.md` §14.
- Skip unknown chunk tags. Fail on a missing required chunk, on a known chunk that appears twice, and on a known chunk with an encoding other than its own.
- Allocate once per array, sized from validated counts. A count is never trusted beyond what the remaining bytes could encode, or for run-length chunks beyond the seat count in `META`.
- Check every cross-reference once, at decode time: every index (string, category, floor, section, row, label rule, object, vertex) is within its table, and every range and partition in §4.2.3 holds. A decoded chart is internally consistent, so consumers index it without further checks. The writer applies the same check and refuses a chart that fails it.
- Return a typed error naming the chunk and the rule broken.
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
