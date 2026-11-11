use std::collections::BTreeMap;

use cavea_core::label::{self, LabelBuf, LabelRule};
use cavea_format::{Artefact, Chart, NONE, decode};

use super::*;
use crate::document::Node;

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `lo..=hi`.
    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
    fn chance(&mut self, percent: i64) -> bool {
        self.int(0, 99) < percent
    }
    fn real(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.int(0, items.len() as i64 - 1) as usize]
    }
}

/// Generates valid chart documents: floors with and without focal points,
/// rotated sections, row blocks with every curve, skew, stagger, removed
/// seats, overrides and label schemes, free-form rows, rows outside sections,
/// explicit and automatic outlines, and every kind of decoration node.
struct Gen {
    r: Rng,
    next: u32,
}

impl Gen {
    fn id(&mut self) -> u32 {
        self.next += 1;
        self.next
    }

    fn doc(seed: u64) -> String {
        let mut g = Gen {
            r: Rng::new(seed),
            next: 100,
        };
        let floors = g.r.int(1, 3);
        let mut floor_json = Vec::new();
        let mut nodes = Vec::new();
        for fl in 1..=floors {
            let focal = if g.r.chance(70) {
                format!(
                    r#","focalPoint":[{},{}]"#,
                    g.r.real(-500.0, 500.0),
                    g.r.real(-1500.0, -500.0)
                )
            } else {
                String::new()
            };
            let level = g.r.int(-1, 2);
            floor_json.push(format!(
                r#"{{"id":{fl},"name":"F{fl}","level":{level}{focal}}}"#
            ));
            for _ in 0..g.r.int(1, 3) {
                nodes.push(g.section(fl));
            }
            if g.r.chance(50) {
                let node = g.bookable(&format!(r#""floor":{fl},"category":"c1","#));
                nodes.push(node);
            }
            if g.r.chance(50) {
                nodes.push(g.decoration());
            }
        }
        let cats: Vec<String> = (0..4)
            .map(|i| {
                format!(
                    r##"{{"key":"c{i}","label":"C{i}","color":"#{i}0A0B0","accessible":{}}}"##,
                    i == 3
                )
            })
            .collect();
        format!(
            r#"{{"schema":1,"name":"gen {seed}","nextOid":{},"categories":[{}],"floors":[{}],"nodes":[{}]}}"#,
            g.next + 1,
            cats.join(","),
            floor_json.join(","),
            nodes.join(",")
        )
    }

    fn place(&mut self) -> String {
        format!(
            r#""origin":[{},{}],"rotation":{}"#,
            self.r.real(-3000.0, 3000.0),
            self.r.real(-3000.0, 3000.0),
            self.r.real(-180.0, 180.0)
        )
    }

    fn section(&mut self, floor: i64) -> String {
        let id = self.id();
        let label = if self.r.chance(80) {
            format!(r#""label":"S{id}","#)
        } else {
            String::new()
        };
        let mut children = Vec::new();
        for _ in 0..self.r.int(0, 2) {
            let category = if self.r.chance(50) {
                r#""category":"c2","#
            } else {
                ""
            };
            children.push(self.bookable(category));
        }
        if self.r.chance(30) {
            children.push(self.decoration());
        }
        if self.r.chance(10) {
            let hidden = self.bookable(r#""hidden":true,"#);
            children.push(hidden);
        }
        let place = self.place();
        let priority = self.r.real(0.5, 2.0);
        // A convex explicit outline, or the automatic one.
        let outline = if self.r.chance(25) {
            let (n, radius) = (self.r.int(3, 12), self.r.real(100.0, 3000.0));
            let pts: Vec<String> = (0..n)
                .map(|k| {
                    let (s, c) = (k as f64 * core::f64::consts::TAU / n as f64).sin_cos();
                    format!("[{},{}]", radius * c, radius * s)
                })
                .collect();
            format!(r#""outline":{{"points":[{}]}},"#, pts.join(","))
        } else {
            String::new()
        };
        format!(
            r#"{{"id":{id},"type":"section","floor":{floor},{label}{outline}{place},"category":"c0","priority":{priority},"children":[{}]}}"#,
            children.join(",")
        )
    }

    /// A valid decoration node of any kind.
    fn decoration(&mut self) -> String {
        let id = self.id();
        let place = self.place();
        let style = format!(
            r##""style":{{"fill":"#A0B0C0","stroke":"#102030","strokeWidth":{},"opacity":{}}},"layer":"{}""##,
            self.r.real(0.0, 20.0),
            self.r.real(0.0, 1.0),
            self.r.pick(&["below", "above"])
        );
        let (w, h) = (self.r.real(1.0, 2000.0), self.r.real(1.0, 2000.0));
        let body = match self.r.int(0, 8) {
            0 => format!(
                r#""type":"shape","kind":"rect","size":[{w},{h}],"cornerRadius":{},{style}"#,
                self.r.real(0.0, 300.0)
            ),
            1 => format!(r#""type":"shape","kind":"ellipse","size":[{w},{h}],{style}"#),
            2 | 3 => {
                let pts: Vec<String> = (0..self.r.int(3, 9))
                    .map(|_| {
                        format!(
                            "[{},{}]",
                            self.r.real(-500.0, 500.0),
                            self.r.real(-500.0, 500.0)
                        )
                    })
                    .collect();
                let kind = self.r.pick(&["polygon", "line"]);
                format!(
                    r#""type":"shape","kind":"{kind}","points":[{}],{style}"#,
                    pts.join(",")
                )
            }
            4 => format!(
                r#""type":"shape","kind":"path","d":"M0 0 C{w} 0 {w} {h} 0 {h} q-50 -50 -10 -100 t20 -30 Z M10 10 h50 v{h} H5 z",{style}"#
            ),
            5 => format!(
                r##""type":"text","text":"T{id}","size":{},"color":"#FFFFFF","align":"{}","minZoom":{}"##,
                self.r.real(1.0, 300.0),
                self.r.pick(&["left", "center", "right"]),
                self.r.real(0.0, 4.0)
            ),
            6 => format!(
                r#""type":"icon","icon":"{}","size":{}"#,
                self.r.pick(&["exit", "lift", "bar"]),
                self.r.real(1.0, 100.0)
            ),
            _ => format!(
                r#""type":"image","asset":"{:032x}","size":[{w},{h}],"opacity":{},"role":"{}""#,
                self.r.next(),
                self.r.real(0.0, 1.0),
                self.r.pick(&["background", "reference"])
            ),
        };
        format!(r#"{{"id":{id},{body},{place}}}"#)
    }

    fn seat_labels(&mut self) -> String {
        let scheme = self.r.pick(&["numeric", "alpha", "odd", "even", "roman"]);
        let direction = self.r.pick(&["ltr", "rtl", "center-out", "serpentine"]);
        let continuous = self.r.chance(30);
        let prefix = if self.r.chance(20) {
            r#","prefix":"s""#
        } else {
            ""
        };
        format!(
            r#""seatLabels":{{"scheme":"{scheme}","direction":"{direction}","continuous":{continuous}{prefix}}}"#
        )
    }

    /// Overrides for some of `oids`: flags, category, label, nudge.
    fn overrides(&mut self, oids: &[u32]) -> String {
        let mut out = Vec::new();
        for &oid in oids {
            if !self.r.chance(15) {
                continue;
            }
            let mut fields = Vec::new();
            if self.r.chance(50) {
                let flags = [
                    "accessible",
                    "companion",
                    "restrictedView",
                    "aisle",
                    "removable",
                ];
                let picked: Vec<String> = flags
                    .iter()
                    .filter(|_| self.r.chance(40))
                    .map(|f| format!("\"{f}\""))
                    .collect();
                fields.push(format!(r#""flags":[{}]"#, picked.join(",")));
            }
            if self.r.chance(30) {
                fields.push(r#""category":"c3""#.into());
            }
            if self.r.chance(30) {
                fields.push(format!(r#""label":"W{oid}""#));
            }
            if self.r.chance(40) {
                let (dx, dy) = (self.r.real(-5.0, 5.0), self.r.real(-5.0, 5.0));
                fields.push(format!(r#""nudge":[{dx},{dy}]"#));
            }
            out.push(format!(r#""{oid}":{{{}}}"#, fields.join(",")));
        }
        format!(r#""seatOverrides":{{{}}}"#, out.join(","))
    }

    /// `[first, count]` runs over `n` fresh oids, in one or two runs.
    fn oids(&mut self, n: u32) -> (String, Vec<u32>) {
        let split = if n > 1 && self.r.chance(30) {
            self.r.int(1, i64::from(n) - 1) as u32
        } else {
            n
        };
        let a = self.next + 1;
        self.next += split;
        let b = self.next + 1 + 50; // a hole in the oid space
        self.next += 50 + (n - split);
        let mut all: Vec<u32> = (a..a + split).collect();
        all.extend(b..b + (n - split));
        let json = if split == n {
            format!("[[{a},{n}]]")
        } else {
            format!("[[{a},{split}],[{b},{}]]", n - split)
        };
        (json, all)
    }

    fn removed(&mut self, n: u32) -> String {
        let picked: Vec<String> = (0..n)
            .filter(|_| self.r.chance(8))
            .map(|k| k.to_string())
            .collect();
        format!(r#""removed":[{}]"#, picked.join(","))
    }

    /// A row block or a free-form row; `extra` is spliced into the node.
    fn bookable(&mut self, extra: &str) -> String {
        if self.r.chance(30) {
            return self.free_row(extra);
        }
        let id = self.id();
        let pitch = self.r.real(45.0, 65.0);
        let row_pitch = self.r.real(80.0, 110.0);
        let curve = match self.r.int(0, 3) {
            0 => r#"{"kind":"none"}"#.to_string(),
            1 => format!(
                r#"{{"kind":"arc","radius":{}}}"#,
                self.r.real(1500.0, 4000.0)
            ),
            2 => format!(
                r#"{{"kind":"arc","radius":{}}}"#,
                -self.r.real(4000.0, 6000.0)
            ),
            _ => format!(
                r#"{{"kind":"bulge","amount":{}}}"#,
                self.r.real(-50.0, 50.0)
            ),
        };
        let align = self.r.pick(&["left", "center", "right"]);
        let stagger = if self.r.chance(30) { 0.5 } else { 0.0 };
        let skew = self.r.real(-20.0, 20.0);
        let scheme = self.r.pick(&["alpha", "numeric", "roman"]);
        let direction = self.r.pick(&["front-to-back", "back-to-front"]);
        let mut rows = Vec::new();
        let mut all = Vec::new();
        for _ in 0..self.r.int(1, 6) {
            let rid = self.id();
            let n = self.r.int(1, 25) as u32;
            let (oids, list) = self.oids(n);
            all.extend(list);
            let removed = self.removed(n);
            rows.push(format!(
                r#"{{"rid":{rid},"seats":{n},"oids":{oids},{removed}}}"#
            ));
        }
        let overrides = self.overrides(&all);
        let seat_labels = self.seat_labels();
        let place = self.place();
        format!(
            r#"{{"id":{id},"type":"rowBlock",{extra}{place},"seatPitch":{pitch},"rowPitch":{row_pitch},"curve":{curve},"align":"{align}","stagger":{stagger},"skew":{skew},"rowLabels":{{"scheme":"{scheme}","direction":"{direction}","prefix":"b{id}"}},{seat_labels},"rows":[{}],{overrides}}}"#,
            rows.join(",")
        )
    }

    fn free_row(&mut self, extra: &str) -> String {
        let id = self.id();
        let n = self.r.int(1, 15) as u32;
        let (oids, list) = self.oids(n);
        let points: Vec<String> = (0..self.r.int(2, 3))
            .map(|i| format!("[{},{}]", i * 300, self.r.real(-100.0, 100.0)))
            .collect();
        let removed = self.removed(n);
        let overrides = self.overrides(&list);
        let seat_labels = self.seat_labels();
        let place = self.place();
        let pitch = self.r.real(40.0, 60.0);
        format!(
            r#"{{"id":{id},"type":"row",{extra}{place},"path":{{"kind":"polyline","points":[{}]}},"seatPitch":{pitch},"seats":{n},"oids":{oids},"label":"R{id}",{seat_labels},{removed},{overrides}}}"#,
            points.join(",")
        )
    }
}

/// What the compiled chart should hold for one seat position, computed
/// directly from the expansion and the document.
#[derive(Debug)]
struct Expected {
    world: [f64; 2],
    node: [f64; 2],
    pitch: f64,
    index: u16,
    removed: bool,
    category: String,
    flags: u8,
    label: String,
}

/// Expands each visible bookable node alone and places it with the platform's
/// trigonometry: an independent path to the same seats.
fn expected(doc: &ChartDocument) -> BTreeMap<u32, Expected> {
    let mut out = BTreeMap::new();
    let place = |p: [f64; 2], origin: [f64; 2], rotation: f64| {
        let (s, c) = rotation.to_radians().sin_cos();
        [
            p[0] * c - p[1] * s + origin[0],
            p[0] * s + p[1] * c + origin[1],
        ]
    };
    let mut visit = |node: &Node, section: Option<&doc::Section>| {
        let mut e = Expansion::new();
        let (origin, rotation) = match node {
            Node::RowBlock(b) if !b.hidden => {
                e.push_row_block(b, doc.settings.alpha_overflow).unwrap();
                (b.origin, b.rotation)
            }
            Node::Row(r) if !r.hidden => {
                e.push_row(r, doc.settings.alpha_overflow).unwrap();
                (r.origin, r.rotation)
            }
            _ => return,
        };
        for s in &e.seats {
            let row = &e.rows[s.row as usize];
            let mut world = place(s.pos, origin, rotation);
            if let Some(sec) = section {
                world = place(world, sec.origin, sec.rotation);
            }
            let mut seat = String::new();
            row.write_seat_label(s, &mut seat).unwrap();
            let category = s
                .category
                .or(section.and_then(|x| x.category.as_deref()))
                .unwrap();
            let section_label = section.and_then(|x| x.label.as_deref()).unwrap_or("");
            let expected = Expected {
                world,
                node: s.pos,
                pitch: row.seat_pitch,
                index: s.index,
                removed: s.removed,
                category: category.to_owned(),
                flags: s.flags,
                label: format!("{section_label}-{}-{seat}", row.label),
            };
            assert!(out.insert(s.oid, expected).is_none());
        }
    };
    for node in &doc.nodes {
        match node {
            Node::Section(s) if !s.hidden => {
                for child in &s.children {
                    visit(child, Some(s));
                }
            }
            other => visit(other, None),
        }
    }
    out
}

/// Every seat's full label, regenerated from the decoded chart alone, as a
/// consumer (renderer, server) does: row rule or `LBLX`, then the template.
fn decoded_labels(c: &Chart) -> Vec<String> {
    let s = |i: u32| {
        if i == NONE {
            ""
        } else {
            c.strings.get(i).unwrap()
        }
    };
    let customs: Vec<Vec<String>> = c
        .label_rules
        .iter()
        .map(|l| {
            (l.custom_first..l.custom_first + l.custom_count)
                .map(|i| s(i).to_owned())
                .collect()
        })
        .collect();
    let exceptions: BTreeMap<u32, u32> = c
        .label_exceptions
        .iter()
        .map(|x| (x.idx, x.label))
        .collect();
    let template = s(c.meta.label_template);
    let mut out = vec![String::new(); c.seat_pos.len()];
    for g in &c.segments {
        let row = &c.rows[g.row as usize];
        let section = if row.section == NONE {
            ""
        } else {
            s(c.sections[row.section as usize].label)
        };
        for k in 0..u32::from(g.seat_count) {
            let idx = g.seat_first + k;
            let mut seat = String::new();
            match exceptions.get(&idx) {
                Some(&x) => seat.push_str(s(x)),
                None => {
                    let l = &c.label_rules[row.label_rule as usize];
                    let rule = LabelRule {
                        scheme: l.scheme,
                        start: l.start,
                        step: l.step,
                        overflow: l.overflow,
                        prefix: s(l.prefix),
                        suffix: s(l.suffix),
                        custom: &customs[row.label_rule as usize],
                    };
                    let position = u32::from(g.position) + k;
                    let ordinal = row.ordinal_base
                        + label::seat_ordinal(
                            row.direction.into(),
                            0,
                            row.positions.into(),
                            position,
                        );
                    rule.write(ordinal, &mut seat).unwrap();
                }
            }
            let mut full = String::new();
            write_full_label(template, section, s(row.label), &seat, &mut full).unwrap();
            out[idx as usize] = full;
        }
    }
    out
}

fn compile_json(json: &str) -> Result<Compiled, CompileError> {
    compile(&ChartDocument::from_json(json.as_bytes()).unwrap())
}

fn findings(json: &str) -> Vec<Finding> {
    match compile_json(json) {
        Err(CompileError::Invalid(f)) => f,
        other => panic!("expected findings, got {other:?}"),
    }
}

/// Decodes both artefacts and checks what holds for every compiled chart.
fn decode_both(c: &Compiled) -> Chart {
    let full = decode(&c.full, Artefact::Full).unwrap();
    let public = decode(&c.public, Artefact::Public).unwrap();
    assert_eq!(
        public,
        Chart {
            full: None,
            ..full.clone()
        }
    );
    for (bytes, hash) in [(&c.public, c.public_hash), (&c.full, c.full_hash)] {
        let digest = blake3::hash(&bytes[cavea_format::HEADER_LEN..]);
        assert_eq!(hash, digest.as_bytes()[..16]);
        assert_eq!(cavea_format::content_hash(bytes), Some(hash));
    }
    full
}

/// 13 §4 round trip: compile generated documents, decode both artefacts, and
/// compare with the expansion computed directly.
#[test]
fn round_trip_matches_direct_expansion() {
    for seed in 0..60 {
        let json = Gen::doc(seed);
        let doc = ChartDocument::from_json(json.as_bytes()).unwrap();
        let compiled = compile(&doc).unwrap_or_else(|e| panic!("seed {seed}: {e}\n{json}"));
        let chart = decode_both(&compiled);
        let full = chart.full.as_ref().unwrap();
        let want = expected(&doc);
        let seats = want.values().filter(|x| !x.removed).count();
        assert_eq!(chart.seat_pos.len(), seats, "seed {seed}");
        let labels = decoded_labels(&chart);
        let mut seen = std::collections::BTreeSet::new();
        for (idx, &oid) in full.oids.iter().enumerate() {
            let x = &want[&oid];
            assert!(!x.removed && seen.insert(oid));
            let [qx, qy] = chart.seat_pos[idx];
            let err = (f64::from(qx) / 4.0 - x.world[0])
                .abs()
                .max((f64::from(qy) / 4.0 - x.world[1]).abs());
            assert!(err <= 0.125 + 1e-6, "seed {seed} oid {oid}: {err}");
            let cat = &chart.categories[usize::from(chart.seat_category[idx])];
            assert_eq!(chart.strings.get(cat.key), Some(x.category.as_str()));
            assert_eq!(chart.seat_flags[idx], x.flags);
            assert_eq!(labels[idx], x.label, "seed {seed} oid {oid}");
        }
        // Segments: runs of consecutive positions whose neighbours are at most
        // 1.5 pitches apart, split wherever a position is removed or the gap
        // is wider.
        let close = |a: &Expected, b: &Expected| {
            let (dx, dy) = (a.node[0] - b.node[0], a.node[1] - b.node[1]);
            (dx * dx + dy * dy).sqrt() <= 1.5 * a.pitch
        };
        let mut prev: Option<(u32, &Expected)> = None;
        for g in &chart.segments {
            for k in 0..u32::from(g.seat_count) {
                let x = &want[&full.oids[(g.seat_first + k) as usize]];
                assert_eq!(u32::from(x.index), u32::from(g.position) + k);
                if k > 0 {
                    assert!(close(prev.unwrap().1, x), "seed {seed}");
                }
                prev = Some((g.row, x));
            }
        }
        for pair in chart.segments.windows(2) {
            let [a, b] = pair else { unreachable!() };
            if a.row == b.row {
                let end = &want[&full.oids[(a.seat_first + u32::from(a.seat_count) - 1) as usize]];
                let next = &want[&full.oids[b.seat_first as usize]];
                assert!(
                    next.index > end.index + 1 || !close(end, next),
                    "seed {seed}"
                );
            }
        }
    }
}

/// A document exercising arcs, skew, rotation, overrides, floors and ranks.
const FIXED: &str = r##"{"schema":1,"name":"fixed","nextOid":9000,
  "categories":[{"key":"a","label":"A","color":"#112233"},{"key":"b","label":"B","color":"#445566","accessible":true}],
  "floors":[{"id":1,"name":"Stalls","level":0,"focalPoint":[0,-800]},{"id":2,"name":"Circle","level":1}],
  "nodes":[
    {"id":10,"type":"section","floor":1,"label":"Orchestra","origin":[120.5,-40.25],"rotation":12.5,"category":"a","priority":1.25,
     "children":[{"id":11,"type":"rowBlock","seatPitch":52.5,"rowPitch":91.3,"skew":7.25,"stagger":0.37,
       "curve":{"kind":"arc","radius":1234.567},"rotation":-3.3,
       "rows":[{"rid":12,"seats":23,"offset":3.1,"oids":[[100,23]]},{"rid":13,"seats":24,"oids":[[200,24]],"removed":[4,5]}],
       "seatOverrides":{"205":{"nudge":[0.3,-1.7],"flags":["aisle"]},"210":{"label":"W1","category":"b"}}}]},
    {"id":20,"type":"section","floor":2,"label":"Circle","rotation":180,
     "children":[{"id":21,"type":"row","path":{"kind":"polyline","points":[[0,0],[100.3,37.1],[160.9,141.7]]},
       "seatPitch":31.7,"seats":9,"oids":[[800,9]],"label":"AA","category":"b",
       "seatLabels":{"scheme":"alpha","direction":"center-out"}}]},
    {"id":30,"type":"section","floor":1,"label":"Box","origin":[-900.5,300],"rotation":-20.25,"category":"a",
     "outline":{"points":[[-50,-60],[700,-60],[760,200],[300,320],[-50,200]]},
     "children":[{"id":31,"type":"row","path":{"kind":"polyline","points":[[0,0],[600,0]]},"seats":11,"oids":[[900,11]],"label":"Z"},
       {"id":32,"type":"text","text":"Box seats","size":40.5,"color":"#202020","origin":[300,250],"rotation":10,"minZoom":0.25},
       {"id":33,"type":"shape","kind":"rect","size":[100,40],"cornerRadius":12.5,"origin":[300,-30],"rotation":5,
        "style":{"fill":"#AA0000","stroke":"#000000","strokeWidth":3,"opacity":0.75}}]},
    {"id":40,"type":"shape","kind":"ellipse","size":[1200,300],"origin":[0,-900],"rotation":3,
     "style":{"fill":"#1F2937","stroke":"#F9FAFB","strokeWidth":8}},
    {"id":41,"type":"shape","kind":"polygon","points":[[0,0],[300,10],[150,260],[60,90]],"origin":[1500,-700],"layer":"above",
     "style":{"fill":"#336699","stroke":"#000000","strokeWidth":2.5,"opacity":0.4}},
    {"id":42,"type":"shape","kind":"line","points":[[0,0],[200,40],[260,-30]],"origin":[-1500,-700],"style":{"stroke":"#123456","strokeWidth":6}},
    {"id":43,"type":"shape","kind":"path","origin":[-200,-1300],"layer":"above",
     "d":"M0 0 L100 0 C150 0 200 50 200 100 S150 200 100 200 Q50 200 20 150 T0 100 Z m300 0 h50 v50 h-50 z",
     "style":{"fill":"#445566","stroke":"#000000","strokeWidth":1.5}},
    {"id":44,"type":"text","text":"STAGE","size":120,"weight":600,"color":"#F9FAFB","align":"center","origin":[0,-900]},
    {"id":45,"type":"icon","icon":"exit","size":80,"origin":[-1200,100],"rotation":-90},
    {"id":46,"type":"image","asset":"00112233445566778899aabbccddeeff","size":[4000,3000],"opacity":0.3,"role":"background","origin":[0,-200]},
    {"id":47,"type":"image","asset":"b3a1","size":[4000,3000],"role":"reference"}]}"##;

/// The same document gives the same bytes, here and in the wasm32 build. It
/// covers a rotated section with an automatic outline, an explicit outline,
/// every shape kind with a stroke, text, an icon and a background image. A
/// change to the stored hash changes every compiled chart. To run it in
/// wasm32 (a failing test traps):
///
/// ```sh
/// cargo test -p cavea-compiler --target wasm32-unknown-unknown --no-run
/// bun -e 'const [w] = process.argv.slice(-1); new WebAssembly.Instance(new WebAssembly.Module(await Bun.file(w).arrayBuffer())).exports.main(0, 0)' target/wasm32-unknown-unknown/debug/deps/cavea_compiler-*.wasm
/// ```
#[test]
fn compiled_bytes_are_deterministic() {
    let a = compile_json(FIXED).unwrap();
    let b = compile_json(FIXED).unwrap();
    assert_eq!((&a.public, &a.full), (&b.public, &b.full));
    decode_both(&a);
    let chart = decode_both(&a);
    let kinds = (
        chart.sections.len(),
        chart.mesh.draws.len(),
        chart.texts.len(),
    );
    assert_eq!(
        (kinds, chart.icons.len(), chart.images.len()),
        ((3, 2, 2), 1, 1)
    );
    let hex: String = a.public_hash.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "cec6bf086691b432823d81fb2d64686c");
}

/// One row of 10: position 3 removed, and position 7 nudged 30 cu right, so
/// the gap 6–7 is 85 cu (> 1.5 × 55) and 7–8 is 25 cu. Position 1 is nudged
/// to exactly 1.5 pitches from 0, which does not split.
#[test]
fn rows_split_at_removed_seats_and_wide_gaps() {
    let json = r##"{"schema":1,"name":"s","nextOid":99,"categories":[{"key":"a","label":"A","color":"#000000"}],
      "nodes":[{"id":1,"type":"rowBlock","category":"a","align":"left","rows":[{"rid":2,"seats":10,"oids":[[10,10]],"removed":[3]}],
        "seatOverrides":{"17":{"nudge":[30,0]},"11":{"nudge":[27.5,0]}}}]}"##;
    let chart = decode_both(&compile_json(json).unwrap());
    let segments: Vec<_> = chart
        .segments
        .iter()
        .map(|g| (g.seat_first, g.seat_count, g.position))
        .collect();
    assert_eq!(segments, [(0, 3, 0), (3, 3, 4), (6, 3, 7)]);
    assert_eq!(chart.rows[0].positions, 10);
    assert_eq!(chart.rows[0].seat_count, 9);
}

fn oid_order(chart: &Chart) -> Vec<u32> {
    chart.full.as_ref().unwrap().oids.clone()
}

/// `idx` order: floors in document order; on each floor its sections, then
/// rows outside sections, in document order; rows in order. Ranks put the
/// lower level first.
#[test]
fn idx_order_follows_floor_section_row_and_ranks_lower_floors_first() {
    let row = |id: u32, extra: &str| {
        format!(
            r#"{{"id":{id},"type":"row",{extra}"path":{{"kind":"polyline","points":[[0,0],[100,0]]}},"seats":2,"oids":[[{},2]],"label":"R{id}","category":"a"}}"#,
            id * 100
        )
    };
    let section = |id: u32, floor: u32, children: &[String]| {
        format!(
            r#"{{"id":{id},"type":"section","floor":{floor},"label":"S{id}","children":[{}]}}"#,
            children.join(",")
        )
    };
    let nodes = [
        row(1, r#""floor":7,"#),
        section(2, 8, &[row(3, ""), row(4, "")]),
        section(5, 7, &[row(6, "")]),
        row(9, r#""floor":8,"#),
        section(11, 8, &[row(12, "")]),
    ];
    let json = format!(
        r##"{{"schema":1,"name":"o","nextOid":999,"categories":[{{"key":"a","label":"A","color":"#000000"}}],
          "floors":[{{"id":7,"name":"Upper","level":2,"focalPoint":[0,-500]}},{{"id":8,"name":"Ground","level":0,"focalPoint":[0,-500]}}],
          "nodes":[{}]}}"##,
        nodes.join(",")
    );
    let chart = decode_both(&compile_json(&json).unwrap());
    // Floor 7 (first listed): section 5, then row 1. Floor 8: sections 2 and
    // 11, then row 9.
    let rows: Vec<u32> = oid_order(&chart).chunks(2).map(|p| p[0] / 100).collect();
    assert_eq!(rows, [6, 1, 3, 4, 12, 9]);
    let floors: Vec<_> = chart
        .floors
        .iter()
        .map(|f| (f.seat_first, f.seat_count))
        .collect();
    assert_eq!(floors, [(0, 4), (4, 8)]);
    let sections: Vec<_> = chart
        .sections
        .iter()
        .map(|s| (s.seat_first, s.seat_count, s.floor))
        .collect();
    assert_eq!(sections, [(0, 2, 0), (4, 4, 1), (8, 2, 1)]);
    assert_eq!(chart.rows[1].section, NONE);
    let ranks = &chart.full.as_ref().unwrap().ranks;
    let (upper, ground) = ranks.split_at(4);
    assert!(ground.iter().max() < upper.iter().min(), "{ranks:?}");
}

/// A section of 6,000 seats in rows of 100 is cut into whole-row chunks of
/// at most 4,096; a small section gets its own chunk.
#[test]
fn large_sections_chunk_by_whole_rows() {
    let mut rows = Vec::new();
    for i in 0..60 {
        rows.push(format!(
            r#"{{"rid":{},"seats":100,"oids":[[{},100]]}}"#,
            10 + i,
            1000 + i * 100
        ));
    }
    let json = format!(
        r##"{{"schema":1,"name":"c","nextOid":99999,"categories":[{{"key":"a","label":"A","color":"#000000"}}],
          "nodes":[{{"id":1,"type":"section","label":"Big","category":"a","children":[{{"id":2,"type":"rowBlock","rows":[{}]}}]}},
                   {{"id":3,"type":"section","label":"Small","category":"a","children":[{{"id":4,"type":"row","path":{{"kind":"polyline","points":[[0,0],[1,0]]}},"seats":10,"oids":[[9000,10]],"label":"A"}}]}}]}}"##,
        rows.join(",")
    );
    let chart = decode_both(&compile_json(&json).unwrap());
    let chunks: Vec<_> = chart
        .chunks
        .iter()
        .map(|c| (c.section, c.seat_first, c.seat_count))
        .collect();
    assert_eq!(chunks, [(0, 0, 4000), (0, 4000, 2000), (1, 6000, 10)]);
    for c in &chart.chunks {
        assert!(chart.rows.iter().any(|r| r.seat_first == c.seat_first));
        let inside = |p: &[i32; 2]| {
            (c.bounds[0]..=c.bounds[2]).contains(&p[0])
                && (c.bounds[1]..=c.bounds[3]).contains(&p[1])
        };
        let first = c.seat_first as usize;
        assert!(
            chart.seat_pos[first..first + c.seat_count as usize]
                .iter()
                .all(inside)
        );
    }
}

/// Rank: nearer the focal point at the same angle ranks better; mirror-image
/// seats tie; ranks span 0…65535. Without a focal point, rank follows rows.
#[test]
fn rank_by_distance_and_angle_and_row_order_fallback() {
    let doc = |focal: &str| {
        format!(
            r##"{{"schema":1,"name":"r","nextOid":9999,"categories":[{{"key":"a","label":"A","color":"#000000"}}],
              "floors":[{{"id":1,"name":"F"{focal}}}],
              "nodes":[{{"id":1,"type":"rowBlock","category":"a","origin":[0,0],
                "rows":[{{"rid":2,"seats":5,"oids":[[100,5]]}},{{"rid":3,"seats":5,"oids":[[200,5]]}},{{"rid":4,"seats":5,"oids":[[300,5]]}}]}}]}}"##
        )
    };
    let chart = decode_both(&compile_json(&doc(r#","focalPoint":[0,-500]"#)).unwrap());
    let r = &chart.full.as_ref().unwrap().ranks;
    // Seat 2 of each row is on the axis (x = 0).
    assert!(r[2] < r[7] && r[7] < r[12]);
    // Mirror images tie; the centre beats the ends of its row.
    assert_eq!(r[0], r[4]);
    assert_eq!(r[1], r[3]);
    assert!(r[2] < r[1] && r[1] < r[0]);
    assert_eq!((r.iter().min(), r.iter().max()), (Some(&0), Some(&65_535)));

    let chart = decode_both(&compile_json(&doc("")).unwrap());
    let r = &chart.full.as_ref().unwrap().ranks;
    assert_eq!(r.as_slice(), [[0; 5], [32_767; 5], [65_535; 5]].concat());
}

#[test]
fn duplicate_labels_and_ids_name_their_nodes() {
    let row = |id: u32, oid: u32, label: &str| {
        format!(
            r#"{{"id":{id},"type":"row","path":{{"kind":"polyline","points":[[0,0],[1,0]]}},"seats":3,"oids":[[{oid},3]],"label":"{label}","category":"a","origin":[0,{id}00]}}"#
        )
    };
    let json = format!(
        r##"{{"schema":1,"name":"d","nextOid":999,"categories":[{{"key":"a","label":"A","color":"#000000"}}],"nodes":[{},{},{}]}}"##,
        row(1, 100, "A"),
        row(2, 200, "B"),
        row(3, 202, "A")
    );
    let mut f: Vec<_> = findings(&json)
        .into_iter()
        .map(|f| (f.code, f.node, f.detail))
        .collect();
    f.sort_by_key(|x| (x.0, x.1, format!("{:?}", x.2)));
    let label = |s: &str| LabelBuf::try_from(s).unwrap();
    assert_eq!(
        f,
        [
            (
                Code::E001,
                Some(3),
                Detail::DuplicateLabel {
                    label: label("-A-1"),
                    first: 1
                }
            ),
            (
                Code::E001,
                Some(3),
                Detail::DuplicateLabel {
                    label: label("-A-2"),
                    first: 1
                }
            ),
            (
                Code::E001,
                Some(3),
                Detail::DuplicateLabel {
                    label: label("-A-3"),
                    first: 1
                }
            ),
            (
                Code::E009,
                Some(3),
                Detail::DuplicateId { id: 202, first: 2 }
            ),
        ]
    );
}

#[test]
fn bookable_objects_this_slice_cannot_compile_are_reported() {
    let json = r##"{"schema":1,"name":"u","nextOid":999,"categories":[{"key":"a","label":"A","color":"#000000"}],
      "nodes":[{"id":60,"type":"booth","size":[200,150],"label":"Box 1","oid":600,"category":"a"},
               {"id":10,"type":"section","children":[
                 {"id":50,"type":"table","shape":"round","radius":90,"seats":8,"oids":[[500,8]]},
                 {"id":70,"type":"area","shape":{"points":[[0,0],[9,0],[9,9]]},"oid":700,"capacity":5},
                 {"id":80,"type":"text","text":"STAGE","size":120,"color":"#FFFFFF"}]}]}"##;
    let f: Vec<_> = findings(json)
        .into_iter()
        .map(|f| (f.code, f.node))
        .collect();
    assert_eq!(
        f,
        [
            (Code::Unsupported, Some(60)),
            (Code::Unsupported, Some(50)),
            (Code::Unsupported, Some(70))
        ]
    );
    // A chart of decoration alone compiles.
    let json = r##"{"schema":1,"name":"t","nextOid":9,"nodes":[{"id":8,"type":"text","text":"STAGE","size":120,"color":"#FFFFFF"}]}"##;
    let chart = decode_both(&compile_json(json).unwrap());
    assert_eq!((chart.seat_pos.len(), chart.texts.len()), (0, 1));
}

/// Whole turns of rotation, however many, place seats as no rotation does.
#[test]
fn whole_turns_do_not_move_seats() {
    let doc = |rotation: &str| {
        format!(
            r##"{{"schema":1,"name":"t","nextOid":99,"categories":[{{"key":"a","label":"A","color":"#000000"}}],
              "nodes":[{{"id":1,"type":"section","rotation":{rotation},"children":[
                {{"id":2,"type":"row","rotation":{rotation},"path":{{"kind":"polyline","points":[[0,0],[55,0]]}},"seats":3,"oids":[[10,3]],"label":"A","category":"a"}}]}}]}}"##
        )
    };
    let pos = |r: &str| decode_both(&compile_json(&doc(r)).unwrap()).seat_pos;
    assert_eq!(pos("0"), [[0, 0], [220, 0], [440, 0]]);
    assert_eq!(pos("36000000000000000"), pos("0"));
    assert_eq!(pos("-720"), pos("0"));
}

/// A row with no positions has no seat labels to check its rule against; it
/// is left out rather than storing a rule the format refuses.
#[test]
fn empty_rows_store_no_label_rule() {
    let prefix = "p".repeat(65);
    let json = format!(
        r##"{{"schema":1,"name":"e","nextOid":99,"categories":[{{"key":"a","label":"A","color":"#000000"}}],
          "nodes":[{{"id":1,"type":"row","path":{{"kind":"polyline","points":[[0,0],[1,0]]}},"seats":0,"oids":[],"label":"A","category":"a",
            "seatLabels":{{"prefix":"{prefix}"}}}}]}}"##
    );
    let chart = decode_both(&compile_json(&json).unwrap());
    assert!(chart.rows.is_empty() && chart.label_rules.is_empty());
}

/// Area covered by the triangles of `indices`.
fn tri_area(chart: &Chart, first: u32, count: u32) -> f64 {
    let v = |i: u32| {
        let p = chart.mesh.vertices[i as usize].pos;
        [f64::from(p[0]), f64::from(p[1])]
    };
    chart.mesh.indices[first as usize..(first + count) as usize]
        .chunks(3)
        .map(|t| {
            let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
            ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
        })
        .sum()
}

/// Section outline ranges, in `SECT` order, then the draw ranges, tile the
/// index buffer.
fn assert_mesh_partition(chart: &Chart) {
    let ranges = chart
        .sections
        .iter()
        .map(|s| (s.mesh_first, s.mesh_count))
        .chain(
            chart
                .mesh
                .draws
                .iter()
                .map(|d| (d.index_first, d.index_count)),
        );
    let mut next = 0;
    for (first, count) in ranges {
        assert_eq!(first, next);
        next += count;
    }
    assert_eq!(next as usize, chart.mesh.indices.len());
    assert_eq!(chart.mesh.indices.len() % 3, 0);
}

/// A chart holding one top-level node.
fn one_node(node: &str) -> Chart {
    let json = format!(r#"{{"schema":1,"name":"m","nextOid":99,"nodes":[{node}]}}"#);
    let chart = decode_both(&compile_json(&json).unwrap_or_else(|e| panic!("{e}")));
    assert_mesh_partition(&chart);
    chart
}

/// Shapes become triangles that cover them: plain areas exactly, curves
/// within the flattening tolerance, strokes at their width with mitred
/// corners. Explicit outlines are tessellated in world space and white.
#[test]
fn meshes_cover_their_shapes() {
    let tol = tessellate::TOLERANCE;
    let c = one_node(
        r##"{"id":1,"type":"shape","kind":"rect","size":[1200,300],"style":{"fill":"#102030"}}"##,
    );
    assert_eq!(c.mesh.draws.len(), 1);
    let d = c.mesh.draws[0];
    assert_eq!((d.layer, d.index_first), (cavea_format::Layer::Below, 0));
    assert_eq!(tri_area(&c, 0, d.index_count), 360_000.0);
    assert!(
        c.mesh
            .vertices
            .iter()
            .all(|v| v.color == [0x10, 0x20, 0x30, 255])
    );

    // A rounded rectangle: corner points on their circles, area short of the
    // true one by at most the arc length × tolerance. Opacity 0.5 is 128.
    let c = one_node(
        r##"{"id":1,"type":"shape","kind":"rect","size":[200,100],"cornerRadius":30,"layer":"above","style":{"fill":"#FF0000","opacity":0.5}}"##,
    );
    assert_eq!(c.mesh.draws[0].layer, cavea_format::Layer::Above);
    for v in &c.mesh.vertices {
        let [x, y] = v.pos.map(f64::from);
        let (cx, cy) = (x.abs() - 70.0, y.abs() - 20.0);
        if cx > 0.0 && cy > 0.0 {
            assert!(((cx * cx + cy * cy).sqrt() - 30.0).abs() < 1e-3, "{x} {y}");
        }
        assert_eq!(v.color, [255, 0, 0, 128]);
    }
    let exact = 20_000.0 - (4.0 - core::f64::consts::PI) * 900.0;
    let got = tri_area(&c, 0, c.mesh.indices.len() as u32);
    assert!(got <= exact && got >= exact - 2.0 * core::f64::consts::PI * 30.0 * tol);

    let c = one_node(
        r##"{"id":1,"type":"shape","kind":"ellipse","size":[600,200],"origin":[0,2000],"style":{"fill":"#00FF00"}}"##,
    );
    for v in &c.mesh.vertices {
        let [x, y] = v.pos.map(f64::from);
        assert!(((x / 300.0).powi(2) + ((y - 2000.0) / 100.0).powi(2) - 1.0).abs() < 1e-5);
    }
    let exact = core::f64::consts::PI * 300.0 * 100.0;
    let got = tri_area(&c, 0, c.mesh.indices.len() as u32);
    assert!(got <= exact && got >= exact - 1_405.0 * tol, "{got}");

    // A stroked line is as wide as its stroke; a stroked square's corners are
    // mitred, not bevelled.
    let c = one_node(
        r##"{"id":1,"type":"shape","kind":"line","points":[[0,0],[100,0]],"origin":[0,3000],"style":{"fill":"#FFFFFF","stroke":"#0000FF","strokeWidth":10}}"##,
    );
    assert!(
        c.mesh
            .vertices
            .iter()
            .all(|v| (v.pos[1] - 3000.0).abs() == 5.0)
    );
    assert_eq!(tri_area(&c, 0, c.mesh.indices.len() as u32), 1_000.0);
    let c = one_node(
        r##"{"id":1,"type":"shape","kind":"rect","size":[100,100],"style":{"stroke":"#000000","strokeWidth":10}}"##,
    );
    let extent = c
        .mesh
        .vertices
        .iter()
        .map(|v| v.pos[0].abs().max(v.pos[1].abs()));
    assert_eq!(extent.fold(0.0f32, f32::max), 55.0);
    assert_eq!(
        tri_area(&c, 0, c.mesh.indices.len() as u32),
        110.0 * 110.0 - 90.0 * 90.0
    );

    // An explicit outline, rotated 90° with its section: the triangles cover
    // it, the bounds cover it, and the anchor is at its centre.
    let json = r##"{"schema":1,"name":"o","nextOid":99,"categories":[{"key":"a","label":"A","color":"#000000"}],
      "nodes":[{"id":1,"type":"section","label":"Box","rotation":90,"origin":[1000,0],"category":"a",
        "outline":{"points":[[0,0],[400,0],[400,300],[0,300]]},
        "children":[{"id":2,"type":"row","path":{"kind":"polyline","points":[[100,150],[300,150]]},"seats":3,"oids":[[10,3]],"label":"A"}]},
        {"id":3,"type":"shape","kind":"rect","size":[10,10],"style":{"fill":"#FFFFFF"}}]}"##;
    let c = decode_both(&compile_json(json).unwrap());
    assert_mesh_partition(&c);
    let s = c.sections[0];
    assert_eq!(tri_area(&c, s.mesh_first, s.mesh_count), 120_000.0);
    let outline = &c.mesh.indices[s.mesh_first as usize..(s.mesh_first + s.mesh_count) as usize];
    for &i in outline {
        let v = c.mesh.vertices[i as usize];
        assert_eq!(v.color, [255; 4]);
        assert!((v.pos[0] - 700.0).abs() < 1e-3 || (v.pos[0] - 1000.0).abs() < 1e-3);
        assert!(v.pos[1].abs() < 1e-3 || (v.pos[1] - 400.0).abs() < 1e-3);
    }
    assert_eq!(s.bounds, [2800, 0, 4000, 1600]);
    assert_eq!(s.label_anchor, [3400, 800]);
    // "Box" in the world's 300 × 400 box: 3 × 0.6 × size < 300 gives 666
    // quarter units.
    assert_eq!(s.label_size, 666);
}

/// Section outline bounds: an automatic outline grows the section's bounds
/// past its seats, and a section without seats has no outline.
#[test]
fn auto_outlines_reach_the_chart() {
    let json = r##"{"schema":1,"name":"o","nextOid":999,"categories":[{"key":"a","label":"A","color":"#000000"}],
      "nodes":[{"id":1,"type":"section","label":"Stalls","rotation":30,"category":"a",
        "children":[{"id":2,"type":"rowBlock","curve":{"kind":"arc","radius":1500},"rows":[{"rid":3,"seats":20,"oids":[[10,20]]},{"rid":4,"seats":20,"oids":[[30,20]],"removed":[3,4]}]}]},
        {"id":5,"type":"section","label":"Empty"}]}"##;
    let c = decode_both(&compile_json(json).unwrap());
    assert_mesh_partition(&c);
    let (s, empty) = (c.sections[0], c.sections[1]);
    assert!(s.mesh_count >= 9 && s.label_size > 0);
    let seat_box = c.rows.iter().fold(c.rows[0].bounds, |b, r| {
        [
            b[0].min(r.bounds[0]),
            b[1].min(r.bounds[1]),
            b[2].max(r.bounds[2]),
            b[3].max(r.bounds[3]),
        ]
    });
    // At least the padding (47.5 cu, 190 quarter units) beyond the seats.
    assert!(s.bounds[0] <= seat_box[0] - 190 && s.bounds[3] >= seat_box[3] + 190);
    assert!(c.mesh.vertices.iter().all(|v| {
        let q = v.pos.map(|x| (x * 4.0) as i32);
        q[0] >= s.bounds[0]
            && q[0] <= s.bounds[2] + 1
            && q[1] >= s.bounds[1]
            && q[1] <= s.bounds[3] + 1
    }));
    let (ax, ay) = (f64::from(s.label_anchor[0]), f64::from(s.label_anchor[1]));
    assert!(
        ax > f64::from(s.bounds[0])
            && ax < f64::from(s.bounds[2])
            && ay > f64::from(s.bounds[1])
            && ay < f64::from(s.bounds[3])
    );
    assert_eq!(
        (
            empty.mesh_count,
            empty.bounds,
            empty.label_anchor,
            empty.label_size
        ),
        (0, [0; 4], [0, 0], 0)
    );
}

/// Text, icons and background images keep their fields through compile and
/// decode, placed through their section; reference images are not compiled.
#[test]
fn decorations_round_trip() {
    let json = r##"{"schema":1,"name":"d","nextOid":99,"nodes":[
      {"id":1,"type":"section","rotation":90,"origin":[100,0],"children":[
        {"id":2,"type":"text","text":"STAGE","size":120,"weight":600,"color":"#F9FAFB","align":"center","minZoom":0.5,"origin":[10,0],"rotation":45}]},
      {"id":3,"type":"icon","icon":"entrance","size":80,"origin":[-50.3,20]},
      {"id":4,"type":"image","asset":"0123456789abcdef0123456789abcdef","size":[4000,3000],"opacity":0.5,"role":"background","origin":[2000,1500]},
      {"id":5,"type":"image","asset":"not a hash","size":[1,1],"role":"reference"}]}"##;
    let c = decode_both(&compile_json(json).unwrap());
    let t = c.texts[0];
    assert_eq!(c.strings.get(t.text), Some("STAGE"));
    assert_eq!(
        (
            t.pos, t.size, t.angle, t.color, t.align, t.weight, t.min_zoom
        ),
        (
            [400, 40],
            480,
            24_576,
            [0xF9, 0xFA, 0xFB, 255],
            cavea_format::TextAlign::Center,
            600,
            0.5
        )
    );
    let i = c.icons[0];
    assert_eq!(c.strings.get(i.key), Some("entrance"));
    assert_eq!((i.pos, i.size, i.angle), ([-201, 80], 320, 0));
    assert_eq!(c.images.len(), 1);
    let im = c.images[0];
    assert_eq!(
        c.strings.get(im.asset),
        Some("0123456789abcdef0123456789abcdef")
    );
    assert_eq!(
        (im.bounds, im.angle, im.opacity),
        ([0, 0, 16_000, 12_000], 0, 128)
    );
}

#[test]
fn bad_decorations_and_outlines_name_their_nodes() {
    let json = r##"{"schema":1,"name":"b","nextOid":99,"nodes":[
      {"id":1,"type":"text","text":"x","size":10,"color":"#GGGGGG"},
      {"id":2,"type":"shape","kind":"rect","size":[0,10],"style":{"fill":"#000000"}},
      {"id":3,"type":"image","asset":"ABCDEF","size":[10,10],"role":"background"},
      {"id":4,"type":"shape","kind":"path","d":"M0 0 A 10 10 0 0 1 20 20","style":{"fill":"#000000"}},
      {"id":5,"type":"section","outline":{"points":[[0,0],[100,100],[100,0],[0,100]]}},
      {"id":6,"type":"shape","kind":"polygon","points":[[0,0],[1,1],[0,1]],"style":{"fill":"red"}},
      {"id":7,"type":"shape","kind":"line","points":[[5,5],[5,5]],"style":{"stroke":"#000000","strokeWidth":1}},
      {"id":8,"type":"text","text":"x","size":0,"color":"#000000"},
      {"id":9,"type":"icon","icon":"bar","size":10,"floor":7},
      {"id":10,"type":"shape","kind":"ellipse","size":[1e300,10],"style":{"fill":"#000000"}},
      {"id":11,"type":"shape","kind":"rect","size":[10,10],"origin":[6e8,0],"style":{"fill":"#000000"}}]}"##;
    let got: Vec<_> = findings(json)
        .into_iter()
        .map(|f| (f.code, f.node))
        .collect();
    // Floors are checked while planning, before outlines and decoration.
    let want: Vec<_> = [
        (Code::E007, 9),
        (Code::E005, 5),
        (Code::E005, 1),
        (Code::E005, 2),
        (Code::E007, 3),
        (Code::E005, 4),
        (Code::E005, 6),
        (Code::E005, 7),
        (Code::E005, 8),
        (Code::E005, 10),
        (Code::E005, 11),
    ]
    .into_iter()
    .map(|(c, n)| (c, Some(n)))
    .collect();
    assert_eq!(got, want);
}

/// Structurally random documents, nonsense included: compiling never panics,
/// and whatever compiles decodes.
#[test]
fn random_documents_never_panic_and_accepted_ones_decode() {
    let mut accepted = 0;
    for seed in 0..400 {
        let mut doc = ChartDocument::from_json(Gen::doc(seed).as_bytes()).unwrap();
        let mut r = Rng::new(seed ^ 0xDEAD);
        for _ in 0..r.int(0, 3) {
            mutate(&mut doc, &mut r);
        }
        match compile(&doc) {
            Ok(c) => {
                decode_both(&c);
                accepted += 1;
            }
            Err(CompileError::Invalid(f)) => assert!(!f.is_empty()),
            Err(e) => panic!("seed {seed}: compiler bug: {e}"),
        }
    }
    assert!(accepted > 50, "{accepted}");
}

fn mutate(doc: &mut ChartDocument, r: &mut Rng) {
    let weird = [
        f64::NAN,
        f64::INFINITY,
        -1e308,
        1e300,
        6e8,
        0.0,
        -0.0,
        1e-300,
        -55.0,
        4.0e9,
    ];
    let w = weird[r.int(0, weird.len() as i64 - 1) as usize];
    let n = r.int(0, 99) as u32;
    match r.int(0, 17) {
        0 => doc.schema = n,
        1 => doc.settings.seat_size = w,
        2 => doc.settings.label_template = [
            "{row}",
            "x",
            "{seat}",
            "{section}{section}{section}{section}{section}{section}{section}{section}{row}{seat}",
            "",
            "{",
        ][(n % 6) as usize]
            .into(),
        3 => doc
            .categories
            .push(doc.categories.first().cloned().unwrap()),
        4 => doc.categories.clear(),
        5 => {
            if let Some(f) = doc.floors.first_mut() {
                f.focal_point = Some([w, 0.0]);
            }
        }
        6 => doc.floors.extend(doc.floors.first().cloned()),
        7 => doc.floors.clear(),
        _ => {
            let k = r.int(0, doc.nodes.len() as i64 - 1) as usize;
            let node = &mut doc.nodes[k];
            let node = match node {
                Node::Section(s) if !s.children.is_empty() && r.chance(60) => {
                    let len = s.children.len() as i64;
                    &mut s.children[r.int(0, len - 1) as usize]
                }
                other => other,
            };
            mutate_node(node, r, w, n);
        }
    }
}

fn mutate_node(node: &mut Node, r: &mut Rng, w: f64, n: u32) {
    // A zigzag that crosses itself many times.
    let zigzag = |n: u32| -> Vec<[f64; 2]> {
        (0..n)
            .map(|k| {
                [
                    f64::from(k % 7) * 100.0,
                    f64::from(k % 5) * 130.0 - f64::from(k),
                ]
            })
            .collect()
    };
    match node {
        Node::Section(s) if r.chance(40) => {
            s.outline = doc::Outline::Points(doc::Points {
                points: match r.int(0, 3) {
                    0 => vec![[0.0, 0.0], [w, 0.0], [0.0, 100.0]],
                    1 => zigzag(n + 4),
                    2 => zigzag(5000),
                    _ => vec![[1.0, 1.0]; (n % 4) as usize],
                },
            })
        }
        Node::Shape(sh) => match r.int(0, 9) {
            0 => sh.size = Some([w, 1.0]),
            1 => sh.corner_radius = Some(w),
            2 => sh.points = Some(vec![[w, 0.0], [0.0, 1.0], [1.0, 1.0]]),
            3 => {
                sh.kind = doc::ShapeKind::Polygon;
                sh.points = Some(zigzag(n * 4 + 3));
                sh.style.fill = Some("#000000".into());
            }
            4 => {
                sh.kind = doc::ShapeKind::Path;
                sh.d = Some(
                    [
                        "M",
                        "M0 0 L",
                        "M0 0 Z Z 1",
                        "M 1e400 0 L 1 1",
                        "M0 0 c 1 2 3",
                        "m0 0 l5 5 5",
                    ][(n % 6) as usize]
                        .into(),
                );
            }
            5 => sh.style.stroke_width = w,
            6 => sh.style.opacity = w,
            7 => {
                sh.kind = doc::ShapeKind::Path;
                sh.d = Some(format!(
                    "M0 0 {}",
                    "C 9e5 0 -9e5 9e5 1 1 ".repeat(n as usize * 4)
                ));
            }
            8 => sh.style.stroke = Some("#12345".into()),
            _ => sh.origin = [w, w],
        },
        Node::Text(t) => match r.int(0, 3) {
            0 => t.size = w,
            1 => t.min_zoom = w,
            2 => t.weight = n as u16 * 20,
            _ => t.rotation = w,
        },
        Node::Icon(i) => i.size = w,
        Node::Image(im) => match r.int(0, 2) {
            0 => im.size = [w, 1.0],
            1 => im.opacity = w,
            _ => im.asset = "f".repeat(n as usize),
        },
        Node::Section(s) => match r.int(0, 5) {
            0 => s.origin = [w, 1.0],
            1 => s.rotation = w,
            2 => s.priority = w,
            3 => s.floor = Some(n),
            4 => s.category = Some("nope".into()),
            _ => s.children.push(
                s.children
                    .first()
                    .cloned()
                    .unwrap_or(Node::Section(s.clone())),
            ),
        },
        Node::RowBlock(b) => match r.int(0, 11) {
            0 => b.origin = [1.0, w],
            1 => b.rotation = w,
            2 => b.seat_pitch = w,
            3 => b.row_pitch = w,
            4 => b.curve = doc::Curve::Arc { radius: w },
            5 => b.rows[0].removed.push(n),
            6 => b.rows[0].oids[0][0] = n,
            7 => b.rows[0].seats = n * 100,
            8 => b.seat_labels.scheme = doc::LabelScheme::Custom,
            9 => b.row_labels.prefix = "x".repeat(n as usize),
            10 => b.category = None,
            _ => b.floor = Some(n),
        },
        Node::Row(row) => match r.int(0, 5) {
            0 => row.seat_pitch = w,
            1 => row.label = Some(String::new()),
            2 => {
                row.path = doc::Path::Polyline {
                    points: vec![[w, 0.0], [0.0, 0.0]],
                }
            }
            3 => row.seats = n,
            4 => row.hidden = true,
            _ => row.seat_labels.custom = vec!["".into(); n as usize],
        },
        _ => {}
    }
}

/// 100,000 seats in 100 rotated, curved sections, the size the slice reports
/// are measured on. Run with
/// `cargo test -p cavea-compiler --release -- --ignored --nocapture compile_100k`;
/// with `CAVEA_DUMP=<path>` set it also writes the public artefact there.
#[test]
#[ignore = "timing, run by hand in release mode"]
fn compile_100k_timing() {
    let mut nodes = Vec::new();
    let mut oid = 100_000u32;
    for s in 0..100 {
        let rows: Vec<String> = (0..10)
            .map(|i| {
                let row = format!(
                    r#"{{"rid":{},"seats":100,"oids":[[{oid},100]]}}"#,
                    s * 100 + i + 1000
                );
                oid += 100;
                row
            })
            .collect();
        nodes.push(format!(
            r#"{{"id":{},"type":"section","label":"S{s}","category":"a","rotation":{},"origin":[{},{}],"children":[{{"id":{},"type":"rowBlock","curve":{{"kind":"arc","radius":3000}},"rows":[{}]}}]}}"#,
            10 + s * 2,
            s * 9,
            (s % 10) * 6000,
            (s / 10) * 3000,
            11 + s * 2,
            rows.join(",")
        ));
    }
    let json = format!(
        r##"{{"schema":1,"name":"100k","nextOid":999999,"categories":[{{"key":"a","label":"A","color":"#000000"}}],
          "floors":[{{"id":1,"name":"F","focalPoint":[30000,-2000]}}],"nodes":[{}]}}"##,
        nodes.join(",")
    );
    let doc = ChartDocument::from_json(json.as_bytes()).unwrap();
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let c = compile(&doc).unwrap();
        best = best.min(t.elapsed().as_secs_f64());
        assert_eq!(c.stats.seats, 100_000);
    }
    let c = compile(&doc).unwrap();
    if let Ok(path) = std::env::var("CAVEA_DUMP") {
        std::fs::write(path, &c.public).unwrap();
    }
    eprintln!(
        "100k seats: best of 5 {:.1} ms; public {} B, full {} B, {:?}",
        best * 1e3,
        c.public.len(),
        c.full.len(),
        c.stats
    );
}
