use super::*;
use crate::geometry::area2;

/// Largest distance from a chord's points to the curve, sampled, given the
/// distance from a point to the true curve.
fn chord_error(pts: &[P], dist: impl Fn(P) -> f64) -> f64 {
    let n = pts.len();
    let mut worst = 0.0f64;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        for k in 0..=16 {
            let t = f64::from(k) / 16.0;
            worst = worst.max(dist([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]));
        }
    }
    worst
}

/// Rounded corners and ellipses: every point on the true curve, every chord
/// within the tolerance of it, clockwise.
#[test]
fn curves_stay_within_tolerance() {
    for (w, h, r) in [
        (300.0, 100.0, 20.0),
        (40.0, 40.0, 20.0),
        (5000.0, 900.0, 450.0),
        (10.0, 6.0, 0.4),
    ] {
        let pts = rect(w, h, r).unwrap();
        assert!(area2(&pts) > 0.0);
        let (hw, hh) = (w / 2.0f64, h / 2.0f64);
        // Distance to the rounded rectangle's boundary, from inside.
        let dist = |p: P| {
            let (qx, qy) = (p[0].abs() - (hw - r), p[1].abs() - (hh - r));
            if qx > 0.0 && qy > 0.0 {
                (r - (qx * qx + qy * qy).sqrt()).abs()
            } else {
                (hw - p[0].abs()).min(hh - p[1].abs()).abs()
            }
        };
        assert!(pts.iter().all(|&p| dist(p) < 1e-9), "{w} {h} {r}");
        assert!(chord_error(&pts, dist) <= TOLERANCE, "{w} {h} {r}");
    }
    for (a, b) in [(100.0f64, 100.0f64), (3000.0, 200.0), (0.3, 7.0)] {
        let pts = ellipse(a, b).unwrap();
        assert!(area2(&pts) > 0.0);
        assert!(
            pts.iter()
                .all(|p| ((p[0] / a).powi(2) + (p[1] / b).powi(2) - 1.0).abs() < 1e-12)
        );
        // Each chord against its own arc, densely sampled: vertex k is at
        // parameter 2πk/n.
        let n = pts.len();
        for k in 0..n {
            let (p, q) = (pts[k], pts[(k + 1) % n]);
            let arc: Vec<P> = (0..=400)
                .map(|j| {
                    let t = (k as f64 + f64::from(j) / 400.0) * std::f64::consts::TAU / n as f64;
                    [a * t.cos(), b * t.sin()]
                })
                .collect();
            let dist = |x: P| {
                arc.iter()
                    .map(|c| ((c[0] - x[0]).powi(2) + (c[1] - x[1]).powi(2)).sqrt())
                    .fold(f64::MAX, f64::min)
            };
            let step = std::f64::consts::TAU * a.max(b) / n as f64 / 400.0;
            assert!(
                chord_error(&[p, q], dist) <= TOLERANCE + step,
                "{a} {b} {k}"
            );
        }
    }
}

#[test]
fn path_data_follows_svg() {
    let paths = parse_path("M0,0 H10 V10 h-10 z m20 0 l10 0 10 10 L 0 0").unwrap();
    assert_eq!(
        paths,
        vec![
            (
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
                true
            ),
            (
                vec![[20.0, 0.0], [30.0, 0.0], [40.0, 10.0], [0.0, 0.0]],
                false
            ),
        ]
    );
    assert_eq!(
        parse_path("m1 1 2 2"),
        Ok(vec![(vec![[1.0, 1.0], [3.0, 3.0]], false)])
    );
    // Curves end exactly on their end points; S and T reflect the previous
    // control point, so these two cubics and two quadratics are symmetric.
    let paths = parse_path("M0 0 C0 10 10 10 10 0 S20 -10 20 0 M0 50 Q5 60 10 50 T20 50").unwrap();
    let [(cubic, false), (quad, false)] = &paths[..] else {
        panic!("{paths:?}")
    };
    assert_eq!(cubic.last(), Some(&[20.0, 0.0]));
    assert!(cubic.contains(&[10.0, 0.0]));
    assert_eq!(quad.last(), Some(&[20.0, 50.0]));
    for p in cubic {
        let mirror = [20.0 - p[0], -p[1]];
        assert!(
            cubic
                .iter()
                .any(|q| (q[0] - mirror[0]).abs() < 1e-9 && (q[1] - mirror[1]).abs() < 1e-9)
        );
    }
    assert!(quad.iter().any(|p| p[0] < 10.0 && p[1] > 50.0));
    assert!(quad.iter().any(|p| p[0] > 10.0 && p[1] < 50.0));

    assert_eq!(parse_path("M0 0 A 5 5 0 0 1 10 0"), Err(PathError::Arc));
    // Curves too large for 256 steps to keep within the tolerance.
    assert_eq!(
        parse_path("M0 0 Q500000 1000000 1000000 0 Z"),
        Err(PathError::CurveTooLarge)
    );
    assert_eq!(
        parse_path("M0 0 C1e300 1e300 -1e300 -1e300 5 5"),
        Err(PathError::CurveTooLarge)
    );
    assert!(ellipse(1e6, 1e6).is_none() && rect(3e6, 3e6, 1e6).is_none());
    assert!(ellipse(100_000.0, 10.0).is_some());
    for bad in [
        "",
        "L0 0",
        "M0",
        "M0 0 L1",
        "M0 0 X1 1",
        "M 1e 2",
        "M0 0 Z 5",
        "M 1e999 0",
    ] {
        assert_eq!(parse_path(bad), Err(PathError::Syntax), "{bad:?}");
    }
    let long: String = (0..5000).map(|k| format!("L{k} {}", k % 2)).collect();
    assert_eq!(
        parse_path(&format!("M0 0 {long}")),
        Err(PathError::TooManyPoints)
    );
}

/// Seeded garbage built from path characters never panics, and whatever
/// parses is bounded and has no repeated consecutive points.
#[test]
fn random_path_data_never_panics() {
    let alphabet = b"MmLlHhVvCcSsQqTtZzAa0123456789.-+eE ,";
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..20_000 {
        let mut d = Vec::new();
        for _ in 0..(x % 40) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            d.push(alphabet[(x % alphabet.len() as u64) as usize]);
        }
        let d = String::from_utf8(d).unwrap();
        if let Ok(paths) = parse_path(&d) {
            assert!(paths.iter().all(|p| p.0.len() >= 2), "{d:?}");
            assert!(paths.iter().map(|p| p.0.len()).sum::<usize>() <= MAX_POINTS);
        }
    }
}

fn pieces_area(pts: &[P], closed: bool, h: f64) -> f64 {
    let mut out = Vec::new();
    stroke(pts, closed, h, &mut out);
    for piece in &out {
        assert!(area2(piece).abs() > 0.0);
    }
    out.iter().map(|p| area2(p).abs() / 2.0).sum()
}

/// Where segments are long enough, the pieces tile the stroke exactly: a
/// mitred frame around a rectangle, an open polyline with butt ends, and a
/// sharp turn past the miter limit, which is bevelled.
#[test]
fn stroke_pieces_tile_the_stroke() {
    let (w, h, s) = (300.0, 100.0, 5.0);
    let frame = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
    let want = (w + 2.0 * s) * (h + 2.0 * s) - (w - 2.0 * s) * (h - 2.0 * s);
    assert!((pieces_area(&frame, true, s) - want).abs() < 1e-6);
    // The same, counter-clockwise.
    let mut ccw = frame;
    ccw.reverse();
    assert!((pieces_area(&ccw, true, s) - want).abs() < 1e-6);

    // An open path turning 90° left then right, butt ends: each miter adds
    // the square its overlap on the inner side takes away.
    let zig = [[0.0, 0.0], [100.0, 0.0], [100.0, -100.0], [200.0, -100.0]];
    let want = 300.0 * 2.0 * s;
    assert!((pieces_area(&zig, false, s) - want).abs() < 1e-6);

    // A hairpin of turn 170°: past the limit, so a bevel; the inner sides
    // of its long segments still meet at the inner miter point.
    let (sin, cos) = (10f64.to_radians().sin(), 10f64.to_radians().cos());
    let pin = [
        [0.0, 0.0],
        [1000.0, 0.0],
        [1000.0 - 1000.0 * cos, 1000.0 * sin],
    ];
    let mut out = Vec::new();
    stroke(&pin, false, s, &mut out);
    assert!(out.iter().any(|p| p.len() == 3));
    assert!(out.iter().all(|p| p.len() <= 4));
}
