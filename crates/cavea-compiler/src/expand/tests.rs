use super::*;
use crate::document::{AlphaOverflow, Node, RowBlock};

const EPS: f64 = 1e-9;

fn block(json: &str) -> RowBlock {
    let node: Node = serde_json::from_str(json).unwrap();
    match node {
        Node::RowBlock(b) => b,
        other => panic!("{other:?}"),
    }
}

/// A block of rows with the given seat counts and consecutive oids from 1000.
fn rows_block(fields: &str, seats: &[u32]) -> RowBlock {
    let mut next = 1000;
    let rows: Vec<String> = seats
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let row = format!(r#"{{"rid":{},"seats":{n},"oids":[[{next},{n}]]}}"#, 100 + i);
            next += n;
            row
        })
        .collect();
    let sep = if fields.is_empty() { "" } else { "," };
    block(&format!(
        r#"{{"id":1,"type":"rowBlock","rows":[{}]{sep}{fields}}}"#,
        rows.join(",")
    ))
}

fn expand(b: &RowBlock) -> Expansion<'_> {
    let mut e = Expansion::new();
    e.push_row_block(b, AlphaOverflow::Double).unwrap();
    e
}

fn error(b: &RowBlock) -> ExpandErrorKind {
    let mut e = Expansion::new();
    let err = e.push_row_block(b, AlphaOverflow::Double).unwrap_err();
    assert!(
        e.rows.is_empty() && e.seats.is_empty(),
        "nothing is kept on error"
    );
    err.kind
}

fn row_seats<'a>(e: &'a Expansion<'_>, row: usize) -> &'a [Seat<'a>] {
    let r = &e.rows[row];
    let start = r.first_seat as usize;
    &e.seats[start..start + usize::from(r.seat_count)]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn labels(e: &Expansion<'_>) -> Vec<(String, String)> {
    e.seats
        .iter()
        .map(|s| {
            let row = &e.rows[s.row as usize];
            let mut seat = String::new();
            row.write_seat_label(s, &mut seat).unwrap();
            (row.label.to_string(), seat)
        })
        .collect()
}

#[test]
fn straight_rows_pitch_alignment_offset_stagger() {
    let xs = |e: &Expansion<'_>, row| {
        row_seats(e, row)
            .iter()
            .map(|s| s.pos[0])
            .collect::<Vec<_>>()
    };
    let centre = rows_block(r#""seatPitch":50,"rowPitch":90,"stagger":0.5"#, &[3, 4, 2]);
    let e = expand(&centre);
    assert_eq!(xs(&e, 0), [-50.0, 0.0, 50.0]);
    assert_eq!(
        xs(&e, 1),
        [-50.0, 0.0, 50.0, 100.0],
        "odd rows shift half a pitch"
    );
    assert_eq!(xs(&e, 2), [-25.0, 25.0]);
    assert!(row_seats(&e, 2).iter().all(|s| s.pos[1] == 180.0));

    let left = rows_block(r#""seatPitch":50,"align":"left""#, &[3]);
    assert_eq!(xs(&expand(&left), 0), [0.0, 50.0, 100.0]);
    let mut right = rows_block(r#""seatPitch":50,"align":"right""#, &[3]);
    right.rows[0].offset = 7.0;
    assert_eq!(xs(&expand(&right), 0), [-93.0, -43.0, 7.0]);
}

#[test]
fn arc_rows_keep_pitch_along_concentric_circles() {
    let b = rows_block(
        r#""seatPitch":55,"rowPitch":95,"curve":{"kind":"arc","radius":800}"#,
        &[31, 40, 7],
    );
    let e = expand(&b);
    for (i, r) in [800.0, 895.0, 990.0].into_iter().enumerate() {
        let seats = row_seats(&e, i);
        let centre = [0.0, -800.0];
        for s in seats {
            assert!((dist(s.pos, centre) - r).abs() < EPS, "on the circle");
        }
        let chord = 2.0 * r * (55.0 / (2.0 * r)).sin();
        for pair in seats.windows(2) {
            assert!((dist(pair[0].pos, pair[1].pos) - chord).abs() < EPS);
        }
        let (first, last) = (seats[0].pos, seats[seats.len() - 1].pos);
        assert!((first[0] + last[0]).abs() < EPS && (first[1] - last[1]).abs() < EPS);
        assert!(first[1] < 95.0 * i as f64, "ends curve towards the front");
    }
    // The middle seat of an odd row sits on the straight row's baseline.
    assert!(dist(row_seats(&e, 0)[15].pos, [0.0, 0.0]) < EPS);

    let negative = rows_block(r#""curve":{"kind":"arc","radius":-1000}"#, &[9]);
    let seats = expand(&negative).seats;
    assert!(
        seats[0].pos[1] > 0.0,
        "a negative radius curves away from the front"
    );

    let too_long = rows_block(
        r#""seatPitch":55,"curve":{"kind":"arc","radius":100}"#,
        &[13],
    );
    assert_eq!(error(&too_long), ExpandErrorKind::ArcRow { rid: 100 });
    let through_zero = rows_block(
        r#""rowPitch":95,"curve":{"kind":"arc","radius":-95}"#,
        &[1, 1],
    );
    assert_eq!(error(&through_zero), ExpandErrorKind::ArcRow { rid: 101 });
}

#[test]
fn bulge_follows_the_formula() {
    let b = rows_block(
        r#""seatPitch":10,"rowPitch":100,"curve":{"kind":"bulge","amount":40}"#,
        &[5, 5],
    );
    let e = expand(&b);
    let ys: Vec<f64> = row_seats(&e, 1).iter().map(|s| s.pos[1]).collect();
    assert_eq!(ys, [100.0, 130.0, 140.0, 130.0, 100.0]);
}

#[test]
fn skew_shears_rows_clockwise() {
    let b = rows_block(
        r#""seatPitch":10,"rowPitch":100,"skew":30,"align":"left""#,
        &[1, 1],
    );
    let e = expand(&b);
    let tan = (30f64).to_radians().tan();
    assert_eq!(e.seats[0].pos, [0.0, 0.0]);
    assert!((e.seats[1].pos[0] + 100.0 * tan).abs() < EPS);
}

#[test]
fn oid_runs_removed_and_overrides() {
    let b = block(
        r#"{"id":1,"type":"rowBlock","seatPitch":10,"align":"left","category":"std",
            "rows":[{"rid":2,"seats":6,"oids":[[50,2],[7,4]],"removed":[2,3]}],
            "seatOverrides":{"51":{"flags":["accessible","aisle"],"category":"access"},
                             "8":{"label":"W1","nudge":[1,-2]},
                             "999":{"flags":["companion"]}}}"#,
    );
    let e = expand(&b);
    let oids: Vec<u32> = e.seats.iter().map(|s| s.oid).collect();
    assert_eq!(oids, [50, 51, 7, 8, 9, 10]);
    let removed: Vec<bool> = e.seats.iter().map(|s| s.removed).collect();
    assert_eq!(removed, [false, false, true, true, false, false]);
    let s = &e.seats;
    assert_eq!(s[4].pos, [40.0, 0.0], "a removed position keeps its place");
    assert_eq!((s[1].flags, s[1].category), (0b1001, Some("access")));
    assert_eq!((s[0].flags, s[0].category), (0, Some("std")));
    assert_eq!((s[3].label, s[3].pos), (Some("W1"), [31.0, -2.0]));
    let seat_labels: Vec<String> = labels(&e).into_iter().map(|(_, s)| s).collect();
    assert_eq!(
        seat_labels,
        ["1", "2", "3", "W1", "5", "6"],
        "removed seats keep their numbers"
    );

    let mut short = b.clone();
    short.rows[0].oids = vec![[50, 5]];
    assert_eq!(error(&short), ExpandErrorKind::OidRuns { rid: 2 });
    short.rows[0].oids = vec![[u32::MAX, 6]];
    assert_eq!(error(&short), ExpandErrorKind::OidRuns { rid: 2 });
    let mut outside = b.clone();
    outside.rows[0].removed = vec![6];
    assert_eq!(
        error(&outside),
        ExpandErrorKind::RemovedOutOfRange {
            rid: 2,
            position: 6
        }
    );
}

#[test]
fn row_and_seat_numbering() {
    // Back to front with continuous serpentine numbering: the back row is "A"
    // and numbered first, left to right; the next row forward continues right
    // to left, and the front row left to right again. Row 1's override keeps its
    // place in the sequence.
    let mut b = rows_block(
        r#""rowLabels":{"scheme":"alpha","direction":"back-to-front","prefix":"R"},
           "seatLabels":{"direction":"serpentine","continuous":true}"#,
        &[2, 3, 2],
    );
    b.rows[1].label = Some("Mid".into());
    let got = labels(&expand(&b));
    let want = [
        ("RC", "6"),
        ("RC", "7"),
        ("Mid", "5"),
        ("Mid", "4"),
        ("Mid", "3"),
        ("RA", "1"),
        ("RA", "2"),
    ];
    assert_eq!(got, want.map(|(r, s)| (r.to_string(), s.to_string())));

    let skip = rows_block(
        r#""rowLabels":{"skip":["B"]},"seatLabels":{"scheme":"even","prefix":"S"}"#,
        &[1, 1],
    );
    assert_eq!(
        labels(&expand(&skip)),
        [("A".into(), "S2".into()), ("C".into(), "S2".into())]
    );
}

#[test]
fn rejects_out_of_limit_and_malformed_blocks() {
    let wide = rows_block("", &[4096]);
    assert_eq!(
        error(&wide),
        ExpandErrorKind::TooManySeatsInRow {
            rid: 100,
            seats: 4096
        }
    );
    // 245 full rows exceed 1,000,000 seats; rejected before anything is allocated.
    let mut huge = rows_block("", &[1]);
    let row = huge.rows[0].clone();
    huge.rows = vec![doc::BlockRow { seats: 4095, ..row }; 245];
    assert_eq!(error(&huge), ExpandErrorKind::TooManyObjects);

    let mut nan = rows_block("", &[3]);
    nan.seat_pitch = f64::NAN;
    assert_eq!(
        error(&nan),
        ExpandErrorKind::InvalidNumber { field: "seatPitch" }
    );
    let mut skew = rows_block("", &[3]);
    skew.skew = 90.0;
    assert_eq!(
        error(&skew),
        ExpandErrorKind::InvalidNumber { field: "skew" }
    );
    let mut far = rows_block("", &[3]);
    far.seat_pitch = 1e308;
    assert_eq!(
        error(&far),
        ExpandErrorKind::InvalidNumber { field: "position" }
    );

    let table_direction = rows_block(r#""seatLabels":{"direction":"clockwise"}"#, &[3]);
    assert_eq!(
        error(&table_direction),
        ExpandErrorKind::DirectionNotForRows
    );
    let bad_start = rows_block(r#""rowLabels":{"start":"A1"}"#, &[3]);
    assert_eq!(error(&bad_start), ExpandErrorKind::InvalidLabelStart);
    let roman = rows_block(r#""seatLabels":{"scheme":"roman","start":3998}"#, &[3]);
    assert_eq!(
        error(&roman),
        ExpandErrorKind::Label {
            rid: 100,
            error: LabelError::OutOfRange
        }
    );
    let custom = rows_block(
        r#""seatLabels":{"scheme":"custom","custom":["a","b"]}"#,
        &[3],
    );
    assert_eq!(
        error(&custom),
        ExpandErrorKind::Label {
            rid: 100,
            error: LabelError::CustomExhausted
        }
    );

    // A failed node leaves earlier nodes in place.
    let small = rows_block("", &[2]);
    let mut e = Expansion::new();
    e.push_row_block(&small, AlphaOverflow::Double).unwrap();
    assert!(e.push_row_block(&wide, AlphaOverflow::Double).is_err());
    assert_eq!((e.rows.len(), e.seats.len()), (1, 2));
}

#[test]
fn free_row_follows_its_polyline() {
    let json = r#"{"id":40,"type":"row","path":{"kind":"polyline","points":[[0,0],[30,40],[30,40],[30,140]]},
        "seatPitch":25,"seats":8,"oids":[[400,8]],"label":"A",
        "seatLabels":{"scheme":"odd","direction":"center-out"}}"#;
    let Node::Row(row) = serde_json::from_str(json).unwrap() else {
        panic!()
    };
    let mut e = Expansion::new();
    e.push_row(&row, AlphaOverflow::Double).unwrap();
    let pos: Vec<[f64; 2]> = e.seats.iter().map(|s| s.pos).collect();
    // The first segment is 50 long; then straight down; seats past the end
    // continue along the last segment.
    let want = [
        [0., 0.],
        [15., 20.],
        [30., 40.],
        [30., 65.],
        [30., 90.],
        [30., 115.],
        [30., 140.],
        [30., 165.],
    ];
    for (got, want) in pos.iter().zip(want) {
        assert!(dist(*got, want) < EPS, "{got:?} {want:?}");
    }
    assert_eq!(e.rows[0].label.as_str(), "A");
    let seat_labels: Vec<String> = labels(&e).into_iter().map(|(_, s)| s).collect();
    assert_eq!(seat_labels, ["13", "9", "5", "1", "3", "7", "11", "15"]);

    let mut flat = row.clone();
    flat.path = doc::Path::Polyline {
        points: vec![[5.0, 5.0], [5.0, 5.0]],
    };
    let mut e = Expansion::new();
    let err = e.push_row(&flat, AlphaOverflow::Double).unwrap_err();
    assert_eq!(
        err,
        ExpandError {
            node: 40,
            kind: ExpandErrorKind::PathTooShort
        }
    );
}
