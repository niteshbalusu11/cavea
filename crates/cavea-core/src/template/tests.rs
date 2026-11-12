#![allow(clippy::unwrap_used, reason = "tests")]

use super::*;
use crate::label::LabelBuf;

fn full(template: &str, section: &str, row: &str, seat: &str) -> String {
    let mut out = String::new();
    write_full_label(template, section, row, seat, &mut out).unwrap();
    out
}

/// The rule before D-96: every placeholder replaced, empty or not.
fn replaced(template: &str, values: [&str; 3]) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (text, tail) = rest.split_at(open);
        out.push_str(text);
        match PARTS.iter().position(|key| tail.starts_with(key)) {
            Some(i) => {
                out.push_str(values[i]);
                rest = &tail[PARTS[i].len()..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out + rest
}

/// The rule as 03 §3.1.1 states it, written the long way: split the template
/// into literals and placeholders, keep the placeholders with a value, and
/// join them with the literal that followed the first of each pair.
fn stated(template: &str, values: [&str; 3]) -> String {
    let (mut literals, mut parts, mut text) = (Vec::new(), Vec::new(), String::new());
    let mut i = 0;
    while i < template.len() {
        let tail = &template[i..];
        if let Some(k) = PARTS.iter().position(|key| tail.starts_with(key)) {
            literals.push(std::mem::take(&mut text));
            parts.push(k);
            i += PARTS[k].len();
        } else {
            let c = tail.chars().next().unwrap();
            text.push(c);
            i += c.len_utf8();
        }
    }
    literals.push(text);
    let kept: Vec<usize> = (0..parts.len())
        .filter(|&j| !values[parts[j]].is_empty())
        .collect();
    let mut out = literals[0].clone();
    for (n, &j) in kept.iter().enumerate() {
        if n > 0 {
            out.push_str(&literals[kept[n - 1] + 1]);
        }
        out.push_str(values[parts[j]]);
    }
    if !parts.is_empty() {
        out.push_str(literals.last().unwrap());
    }
    out
}

/// SplitMix64, so a failure is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[(self.next() % from.len() as u64) as usize]
    }
}

const PLACEHOLDERS: [&str; 3] = ["{section}", "{row}", "{seat}"];
const LITERALS: [&str; 14] = [
    "-", "-", " / ", "·", "", "{", "}", "{x}", "Seat ", "é", "{sec", "{{", "--", "Ω ",
];
const VALUES: [&str; 8] = ["A", "Stalls", "10", "Ω", "x-y", "{row}", "Box 1", "é"];

/// A template of up to five placeholders with literals between, before and
/// after them, each possibly empty.
fn template(r: &mut Rng) -> String {
    let n = r.next() % 6;
    let mut t = String::new();
    if r.next() % 3 == 0 {
        t.push_str(r.pick(&LITERALS));
    }
    for i in 0..n {
        if i > 0 {
            t.push_str(r.pick(&LITERALS));
        }
        t.push_str(r.pick(&PLACEHOLDERS));
    }
    if r.next() % 3 == 0 {
        t.push_str(r.pick(&LITERALS));
    }
    t
}

fn values(r: &mut Rng, empty_one_in: u64) -> [&'static str; 3] {
    let mut v = || {
        if empty_one_in > 0 && r.next() % empty_one_in == 0 {
            ""
        } else {
            r.pick(&VALUES)
        }
    };
    [v(), v(), v()]
}

#[test]
fn the_specs_examples() {
    let d = "{section}-{row}-{seat}";
    let slash = "{section} / {row} / {seat}";
    let prefix = "Seat {row}{seat}";
    let suffix = "{section}-{row}-{seat} (box)";
    let cases = [
        (d, ["Stalls", "C", "12"], "Stalls-C-12"),
        (d, ["", "D", "10"], "D-10"),
        (d, ["Stalls", "", "12"], "Stalls-12"),
        (d, ["Stalls", "C", ""], "Stalls-C"),
        (d, ["", "", "12"], "12"),
        (d, ["", "", ""], ""),
        (slash, ["", "C", "12"], "C / 12"),
        (slash, ["Circle", "", "7"], "Circle / 7"),
        ("{row}{seat}", ["", "C", "12"], "C12"),
        (prefix, ["", "C", "12"], "Seat C12"),
        (prefix, ["", "", "12"], "Seat 12"),
        (suffix, ["", "C", "12"], "C-12 (box)"),
        (suffix, ["", "", ""], " (box)"),
        ("{section}/{row}-{seat}", ["", "C", "12"], "C-12"),
        ("{section}/{row}-{seat}", ["Box", "", "j"], "Box/j"),
        ("{section}·{row}·{seat}", ["", "C", "12"], "C·12"),
        ("{row}/{seat} @{section}{x}", ["", "C", "12"], "C/12{x}"),
        ("{sec{section}-{row}", ["", "C", "12"], "{secC"),
        ("no placeholders", ["", "", ""], "no placeholders"),
    ];
    for (template, [s, r, n], want) in cases {
        assert_eq!(
            full(template, s, r, n),
            want,
            "{template} {s:?} {r:?} {n:?}"
        );
        assert_eq!(stated(template, [s, r, n]), want, "{template}");
    }
}

/// Labels whose parts are all non-empty are exactly what they were before.
#[test]
fn full_parts_compose_as_before() {
    let mut r = Rng(1);
    for _ in 0..20_000 {
        let t = template(&mut r);
        let v = values(&mut r, 0);
        assert_eq!(full(&t, v[0], v[1], v[2]), replaced(&t, v), "{t:?} {v:?}");
    }
}

/// With any parts empty, the label is the stated rule; with one separator
/// between placeholders and no prefix or suffix, it never starts, ends or
/// doubles that separator because a part was empty, and the non-empty parts
/// appear in order, unaltered.
#[test]
fn empty_parts_take_their_separator() {
    let mut r = Rng(2);
    for _ in 0..20_000 {
        let t = template(&mut r);
        let v = values(&mut r, 2);
        assert_eq!(full(&t, v[0], v[1], v[2]), stated(&t, v), "{t:?} {v:?}");
        // Composing is a function of the template and the parts.
        assert_eq!(full(&t, v[0], v[1], v[2]), full(&t, v[0], v[1], v[2]));
    }
    for sep in ["-", " / ", "·", "--"] {
        for _ in 0..2_000 {
            let n = 1 + r.next() % 5;
            let picks: Vec<usize> = (0..n).map(|_| (r.next() % 3) as usize).collect();
            let t = picks
                .iter()
                .map(|&k| PLACEHOLDERS[k])
                .collect::<Vec<_>>()
                .join(sep);
            let v = [r.pick(&["", "A"]), r.pick(&["", "C"]), r.pick(&["", "12"])];
            let out = full(&t, v[0], v[1], v[2]);
            let kept: Vec<&str> = picks
                .iter()
                .map(|&k| v[k])
                .filter(|x| !x.is_empty())
                .collect();
            assert_eq!(out, kept.join(sep), "{t:?} {v:?}");
            assert!(!out.starts_with(sep) && !out.ends_with(sep), "{t:?} {v:?}");
            assert!(!out.contains(&format!("{sep}{sep}")), "{t:?} {v:?}");
        }
    }
}

/// Hostile templates: unclosed and stray braces, unknown placeholders, long
/// literals, non-ASCII text cut anywhere. Composing never panics, and into a
/// `LabelBuf` it fails past 64 bytes rather than cutting a character.
#[test]
fn hostile_templates_never_panic() {
    let pieces = [
        "{",
        "}",
        "{section",
        "section}",
        "{{row}}",
        "{seat}",
        "{row}",
        "{section}",
        "é",
        "Ω",
        "🎭",
        "{x}",
        "-",
        "\u{0}",
        "\u{301}",
        "{SEAT}",
        "{ row }",
    ];
    let long = "é".repeat(40);
    let mut r = Rng(3);
    for _ in 0..20_000 {
        let mut t = String::new();
        for _ in 0..r.next() % 12 {
            t.push_str(r.pick(&pieces));
        }
        if r.next() % 10 == 0 {
            t.push_str(&long);
        }
        let v = values(&mut r, 2);
        let out = full(&t, v[0], v[1], v[2]);
        assert_eq!(out, stated(&t, v), "{t:?} {v:?}");
        let mut buf = LabelBuf::new();
        match write_full_label(&t, v[0], v[1], v[2], &mut buf) {
            Ok(()) => assert_eq!(buf.as_str(), out),
            Err(_) => assert!(out.len() > 64, "{t:?} {v:?}"),
        }
    }
}

/// The cases the console's TypeScript port is checked against
/// (`apps/console/src/console/manager/labels.test.ts`). `CAVEA_BLESS=1 cargo
/// test -p cavea-core template` rewrites the table; the test fails while it is
/// stale.
#[test]
fn cases_match_the_table() {
    fn json(s: &str) -> String {
        let mut out = String::from('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
                c => out.push(c),
            }
        }
        out + "\""
    }
    let mut r = Rng(4);
    let mut cases: Vec<(String, [&str; 3])> = vec![
        ("{section}-{row}-{seat}".into(), ["", "D", "10"]),
        ("{section}-{row}-{seat}".into(), ["Stalls", "D", "10"]),
        ("Seat {row}{seat}".into(), ["", "", "12"]),
        ("{row}/{seat} @{section}{x}".into(), ["", "C", "12"]),
        ("{sec{section}-{row}".into(), ["", "C", "12"]),
        ("🎭{section}\u{0}{row}".into(), ["Ω", "", "1"]),
    ];
    for _ in 0..200 {
        let t = template(&mut r);
        cases.push((t, values(&mut r, 2)));
    }
    let mut table = String::from(
        "{\n  \"made\": \"CAVEA_BLESS=1 cargo test -p cavea-core template (crates/cavea-core/src/template/tests.rs)\",\n  \"columns\": [\"template\", \"section\", \"row\", \"seat\", \"label\"],\n  \"cases\": [\n",
    );
    for (n, (t, v)) in cases.iter().enumerate() {
        let row = [t.as_str(), v[0], v[1], v[2], &full(t, v[0], v[1], v[2])]
            .map(json)
            .join(", ");
        let comma = if n + 1 < cases.len() { "," } else { "" };
        table.push_str(&format!("    [{row}]{comma}\n"));
    }
    table.push_str("  ]\n}\n");
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../apps/console/src/console/manager/fixtures/label-cases.json"
    );
    if std::env::var_os("CAVEA_BLESS").is_some() {
        std::fs::write(path, &table).unwrap();
    }
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == table,
        "{path} is stale: run CAVEA_BLESS=1 cargo test -p cavea-core template"
    );
}
