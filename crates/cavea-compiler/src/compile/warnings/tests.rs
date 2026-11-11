use crate::compile::tests::compile_json;
use crate::finding::{Code, Detail, Finding};

use super::*;

/// One floor with a focal point, an accessible category used by every seat,
/// a labelled section of two rows of three: no warning. `{cats}`, `{floor}`,
/// `{section}`, `{block}` and `{extra}` are spliced in by each case.
fn doc(cats: &str, floor: &str, section: &str, block: &str, extra: &str) -> String {
    format!(
        r##"{{"schema":1,"name":"w","nextOid":999,
          "categories":[{{"key":"a","label":"A","color":"#2563EB","accessible":true}}{cats}],
          "floors":[{{"id":1,"name":"F"{floor}}}],
          "nodes":[{{"id":1,"type":"section","category":"a"{section},"children":[
            {{"id":2,"type":"rowBlock"{block},"rows":[{{"rid":3,"seats":3,"oids":[[10,3]]}},{{"rid":4,"seats":3,"oids":[[20,3]]}}]}}]}}{extra}]}}"##
    )
}

fn warnings(json: &str) -> Vec<Finding> {
    compile_json(json)
        .unwrap_or_else(|e| panic!("{e}\n{json}"))
        .findings
}

const FOCAL: &str = r#","focalPoint":[0,-500]"#;
const LABEL: &str = r#","label":"S""#;

/// Each warning code on a minimal document that triggers exactly it, with
/// the node it names and its detail; the base document has none.
#[test]
fn each_warning_names_its_node() {
    let base = doc("", FOCAL, LABEL, "", "");
    assert_eq!(warnings(&base), []);
    let cases = [
        (
            doc("", FOCAL, LABEL, r#","seatPitch":20"#, ""),
            Code::W101,
            Some(2),
            Detail::Overlap { other: 2 },
        ),
        (
            doc("", "", LABEL, "", ""),
            Code::W102,
            None,
            Detail::NoFocalPoint { floor: Some(1) },
        ),
        (
            doc("", FOCAL, LABEL, "", "").replace(
                r#""seats":3,"oids":[[10,3]]"#,
                r#""seats":1,"oids":[[10,1]]"#,
            ),
            Code::W103,
            Some(2),
            Detail::SingleSeat { rid: 3 },
        ),
        (
            doc(
                "",
                FOCAL,
                r#","label":"S","outline":{"points":[[-10,-10],[10,-10],[10,10],[-10,10]]}"#,
                "",
                "",
            ),
            Code::W104,
            Some(2),
            Detail::OutsideOutline {
                section: 1,
                rid: 3,
                position: 0,
            },
        ),
        (
            doc(
                r##",{"key":"b","label":"B","color":"#B45309"}"##,
                FOCAL,
                LABEL,
                "",
                "",
            ),
            Code::W105,
            None,
            Detail::UnusedCategory { key: "b".into() },
        ),
        (
            doc("", FOCAL, LABEL, "", "").replace(r#""accessible":true"#, r#""accessible":false"#),
            Code::W106,
            None,
            Detail::NoAccessibleSeat,
        ),
        (
            doc(
                r##",{"key":"b","label":"B","color":"#2A66EB"}"##,
                FOCAL,
                LABEL,
                r#","seatOverrides":{"10":{"category":"b"}}"#,
                "",
            ),
            Code::W107,
            None,
            Detail::SimilarColors {
                key: "b".into(),
                other: "a".into(),
            },
        ),
        (
            doc("", FOCAL, LABEL, "", "").replace("#2563EB", "#FACC15"),
            Code::W107,
            None,
            Detail::LowContrast {
                key: "a".into(),
                background: "light",
                ratio: 146,
            },
        ),
        (
            doc(
                "",
                FOCAL,
                LABEL,
                "",
                r##",{"id":7,"type":"text","text":"Exit","size":1,"color":"#000000"}"##,
            ),
            Code::W108,
            Some(7),
            Detail::TextTooSmall { px: 265 },
        ),
        (
            doc("", FOCAL, "", "", ""),
            Code::W109,
            Some(1),
            Detail::NoSectionLabel,
        ),
        (
            doc(
                "",
                FOCAL,
                LABEL,
                "",
                r#",{"id":7,"type":"icon","icon":"bar","origin":[300000,0],"size":50}"#,
            ),
            Code::W110,
            None,
            Detail::ChartTooLarge { extent: 300_159 },
        ),
    ];
    for (json, code, node, detail) in cases {
        let got = warnings(&json);
        assert_eq!(got, [Finding { code, node, detail }], "{json}");
    }
    // The nearby documents that do not warn: a larger text, an outline round
    // every seat.
    let fine = [
        doc(
            "",
            FOCAL,
            LABEL,
            "",
            r##",{"id":7,"type":"text","text":"Exit","size":4,"color":"#000000"}"##,
        ),
        doc(
            "",
            FOCAL,
            r#","label":"S","outline":{"points":[[-90,-30],[90,-30],[90,130],[-90,130]]}"#,
            "",
            "",
        ),
    ];
    for json in fine {
        assert_eq!(warnings(&json), [], "{json}");
    }
}

/// Warnings come in code order, then in each code's own order, and at most
/// 100 overlap findings, one per node.
#[test]
fn warnings_are_ordered_and_overlaps_capped() {
    let rows: Vec<String> = (0..150)
        .map(|i| {
            format!(
                r#",{{"id":{},"type":"row","floor":1,"category":"a","label":"R{i}","origin":[0,{}],"path":{{"kind":"polyline","points":[[0,0],[10,0]]}},"seatPitch":5,"seats":2,"oids":[[{},2]]}}"#,
                1000 + i,
                i * 100 + 1000,
                5000 + 2 * i
            )
        })
        .collect();
    let json = doc(
        r##",{"key":"b","label":"B","color":"#B45309"}"##,
        "",
        "",
        "",
        &rows.concat(),
    );
    let codes: Vec<(Code, Option<u32>)> =
        warnings(&json).iter().map(|f| (f.code, f.node)).collect();
    let mut want: Vec<(Code, Option<u32>)> = (1000..1100).map(|n| (Code::W101, Some(n))).collect();
    want.extend([
        (Code::W102, None),
        (Code::W105, None),
        (Code::W109, Some(1)),
    ]);
    assert_eq!(codes, want);
}

/// The luminance curve without `powf` matches it to `f64` rounding.
#[test]
fn luminance_matches_the_srgb_curve() {
    for v in 0..=255u8 {
        let c = f64::from(v) / 255.0;
        let exact = if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
        assert!((linear(v) - exact).abs() <= 1e-15, "{v}");
    }
}
