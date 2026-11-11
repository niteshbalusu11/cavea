//! `cavea_format::full_label` against the compiler. It lives here because
//! `cavea-format` may not depend on the compiler, even for tests (01 §3).

use std::collections::BTreeMap;

use cavea_compiler::{
    Expansion,
    document::{
        AlphaOverflow, BlockRow, LabelScheme, LabelStart, Node, RowBlock, SeatDirection,
        SeatOverride,
    },
    write_full_label,
};
use cavea_core::label::LabelBuf;
use cavea_format::{
    Artefact,
    full_label::{LabelIndex, full_label},
};

/// The test kit's stadium with a different seat scheme in each section,
/// removed positions, label overrides, a section without a label, a row block
/// outside any section and a template with stray braces.
fn varied() -> cavea_compiler::ChartDocument {
    let mut doc = cavea_testkit::synthetic::stadium(6_000, 11);
    doc.settings.label_template = "{row}/{seat} @{section}{x}".into();
    let custom: Vec<String> = (0..400).map(|i| format!("c{i}")).collect();
    let mut next = doc.next_oid;
    for (i, node) in doc.nodes.iter_mut().enumerate() {
        let Node::Section(section) = node else {
            continue;
        };
        if i == 3 {
            section.label = None;
        }
        let Some(Node::RowBlock(block)) = section.children.first_mut() else {
            continue;
        };
        let labels = &mut block.seat_labels;
        let (scheme, direction) = match i % 6 {
            0 => (LabelScheme::Numeric, SeatDirection::Rtl),
            1 => (LabelScheme::Odd, SeatDirection::CenterOut),
            2 => (LabelScheme::Alpha, SeatDirection::Serpentine),
            3 => (LabelScheme::Roman, SeatDirection::Ltr),
            4 => (LabelScheme::Custom, SeatDirection::CenterOut),
            _ => (LabelScheme::Even, SeatDirection::Ltr),
        };
        labels.scheme = scheme;
        labels.direction = direction;
        labels.continuous = i % 4 == 1;
        if scheme == LabelScheme::Odd {
            labels.start = Some(LabelStart::Number(3));
        }
        if scheme == LabelScheme::Custom {
            labels.custom = custom.clone();
        }
        if i % 5 == 0 {
            labels.prefix = "s".into();
            labels.suffix = "!".into();
        }
        if i == 3 {
            block.row_labels.prefix = "N".into();
        }
        let rows: &mut Vec<BlockRow> = &mut block.rows;
        for (r, row) in rows.iter_mut().enumerate() {
            if r % 3 == 1 && row.seats > 4 {
                row.removed = vec![1, row.seats - 2];
            }
            if r % 4 == 0 {
                let oid = row.oids[0][0] + 2;
                let label = Some(format!("x{oid}"));
                let seat = SeatOverride {
                    flags: Vec::new(),
                    category: None,
                    label,
                    nudge: None,
                };
                block.seat_overrides.insert(oid, seat);
            }
        }
    }
    // A block of two rows outside any section, far from the bowl.
    let rows = (0..2)
        .map(|r| {
            let row = BlockRow {
                rid: next,
                seats: 8,
                offset: 0.0,
                oids: vec![[next + 1, 8]],
                label: Some(format!("L{r}")),
                removed: Vec::new(),
            };
            next += 9;
            row
        })
        .collect();
    let Some(Node::Section(template)) = doc.nodes.first() else {
        panic!("stadium has sections");
    };
    let Some(Node::RowBlock(block)) = template.children.first() else {
        panic!("section has a block");
    };
    doc.nodes.push(Node::RowBlock(RowBlock {
        id: next,
        origin: [0.0, 9000.0],
        rotation: 0.0,
        curve: cavea_compiler::document::Curve::None {},
        category: Some("premium".into()),
        rows,
        seat_overrides: BTreeMap::new(),
        ..block.clone()
    }));
    doc.next_oid = next + 1;
    doc
}

#[test]
fn full_labels_agree_with_the_compiler_for_every_seat() {
    let doc = varied();
    let compiled = cavea_compiler::compile(&doc).unwrap();
    let chart = cavea_format::decode(&compiled.full, Artefact::Full).unwrap();
    let oids = &chart.full.as_ref().unwrap().oids;
    let idx_of: BTreeMap<u32, u32> = (0u32..).zip(oids).map(|(i, &oid)| (oid, i)).collect();

    // The compiler's own labels: its expansion of each block, composed with
    // its `write_full_label`, keyed by oid.
    let template = &doc.settings.label_template;
    let mut expected = BTreeMap::new();
    let mut add = |block: &RowBlock, section: &str| {
        let mut expansion = Expansion::new();
        expansion
            .push_row_block(block, AlphaOverflow::Double)
            .unwrap();
        for seat in expansion.seats.iter().filter(|s| !s.removed) {
            let row = &expansion.rows[seat.row as usize];
            let mut label = LabelBuf::new();
            row.write_seat_label(seat, &mut label).unwrap();
            let mut full = String::new();
            write_full_label(
                template,
                section,
                row.label.as_str(),
                label.as_str(),
                &mut full,
            )
            .unwrap();
            expected.insert(seat.oid, full);
        }
    };
    for node in &doc.nodes {
        match node {
            Node::Section(s) => {
                for child in &s.children {
                    if let Node::RowBlock(block) = child {
                        add(block, s.label.as_deref().unwrap_or(""));
                    }
                }
            }
            Node::RowBlock(block) => add(block, ""),
            _ => {}
        }
    }

    assert_eq!(expected.len(), chart.meta.seat_count as usize);
    let index = LabelIndex::build(&chart);
    for (oid, label) in &expected {
        let idx = idx_of[oid];
        assert_eq!(
            full_label(&chart, idx).unwrap().as_str(),
            label,
            "oid {oid}"
        );
        assert_eq!(index.find(&chart, label), Some(idx), "{label}");
    }
    // Every scheme took part, and the odd cases.
    let all: Vec<&String> = expected.values().collect();
    for needle in ["/s", "!", " @{x}", "/x", "/c", "/II", "L1/s8! @{x}"] {
        assert!(all.iter().any(|l| l.contains(needle)), "{needle}");
    }
    assert_eq!(index.find(&chart, "nope"), None);
    assert_eq!(index.find(&chart, ""), None);
    assert!(full_label(&chart, chart.meta.seat_count).is_err());
}
