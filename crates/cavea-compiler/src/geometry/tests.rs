use super::*;
use crate::document::{AlphaOverflow, Node};
use crate::expand::Expansion;

/// Seats of row blocks given as JSON, each placed by its own origin and
/// rotation (platform trigonometry: this is the test's own path).
fn seats(blocks: &[String]) -> Vec<P> {
    let mut out = Vec::new();
    for json in blocks {
        let Node::RowBlock(b) = serde_json::from_str::<Node>(json).unwrap() else {
            panic!("not a row block")
        };
        let mut e = Expansion::new();
        e.push_row_block(&b, AlphaOverflow::Double).unwrap();
        let (s, c) = b.rotation.to_radians().sin_cos();
        for seat in e.seats.iter().filter(|s| !s.removed) {
            let [x, y] = seat.pos;
            out.push([x * c - y * s + b.origin[0], x * s + y * c + b.origin[1]]);
        }
    }
    out
}

fn block(extra: &str, rows: &[(u32, &str)]) -> String {
    let mut oid = 1000;
    let rows: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, (n, more))| {
            let row = format!(
                r#"{{"rid":{},"seats":{n},"oids":[[{oid},{n}]]{more}}}"#,
                i + 1
            );
            oid += n;
            row
        })
        .collect();
    format!(
        r#"{{"id":1,"type":"rowBlock",{extra}"rows":[{}]}}"#,
        rows.join(",")
    )
}

/// Distance from `p` to the polygon's boundary.
fn boundary_distance(poly: &[P], p: P) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let t =
                (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            ((a[0] + t * dx - p[0]).powi(2) + (a[1] + t * dy - p[1]).powi(2)).sqrt()
        })
        .fold(f64::INFINITY, f64::min)
}

/// Winding number of the polygon around `p`.
fn winding(poly: &[P], p: P) -> i32 {
    let n = poly.len();
    let mut w = 0;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let side = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
        if a[1] <= p[1] && b[1] > p[1] && side > 0.0 {
            w += 1;
        } else if a[1] > p[1] && b[1] <= p[1] && side < 0.0 {
            w -= 1;
        }
    }
    w
}

/// No two non-neighbouring edges meet; neighbours meet only at their shared
/// point. Solves for the intersection parameters directly.
fn simple(poly: &[P]) -> bool {
    let n = poly.len();
    let seg = |i: usize| (poly[i], poly[(i + 1) % n]);
    for i in 0..n {
        for j in i + 1..n {
            let ((a, b), (c, d)) = (seg(i), seg(j));
            let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
            let den = r[0] * s[1] - r[1] * s[0];
            let ac = [c[0] - a[0], c[1] - a[1]];
            let neighbours = j == i + 1 || (i == 0 && j == n - 1);
            if den == 0.0 {
                // Parallel: collinear overlap is a fold or a touch.
                let collinear = ac[0] * r[1] - ac[1] * r[0] == 0.0;
                let rr = r[0] * r[0] + r[1] * r[1];
                let t0 = (ac[0] * r[0] + ac[1] * r[1]) / rr;
                let t1 = t0 + (s[0] * r[0] + s[1] * r[1]) / rr;
                let (lo, hi) = (t0.min(t1), t0.max(t1));
                let overlap = hi.min(1.0) - lo.max(0.0);
                if collinear && (overlap > 0.0 || (!neighbours && overlap == 0.0)) {
                    return false;
                }
                continue;
            }
            let t = (ac[0] * s[1] - ac[1] * s[0]) / den;
            let u = (ac[0] * r[1] - ac[1] * r[0]) / den;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                let shared = neighbours && ((t == 1.0 && u == 0.0) || (t == 0.0 && u == 1.0));
                if !shared {
                    return false;
                }
            }
        }
    }
    true
}

/// Signed area (shoelace).
fn area(poly: &[P]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

fn hull_area_perimeter(pts: &[P]) -> (f64, f64) {
    let h = convex_hull(pts.to_vec());
    let n = h.len();
    let per = (0..n)
        .map(|i| {
            let (a, b) = (h[i], h[(i + 1) % n]);
            ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
        })
        .sum();
    (area(&h).abs(), per)
}

/// Outline properties over straight, curved, staggered, skewed, multi-block,
/// rotated sections and sections with removed seats: a simple polygon around
/// every seat with at least the padding, no larger than the seats' convex hull
/// grown by five paddings, and following concave blocks.
#[test]
fn auto_outlines_are_simple_padded_and_tight() {
    let rows = |n: usize, seats: u32| vec![(seats, ""); n];
    let removed = [
        (30, ""),
        (30, r#","removed":[0,1,2,14,15,29]"#),
        (30, r#","removed":[10,11,12,13,14,15,16,17,18,19]"#),
        (24, ""),
    ];
    let cases: Vec<(&str, Vec<String>, f64)> = vec![
        ("straight", vec![block("", &rows(8, 20))], 47.5),
        (
            "arc",
            vec![block(
                r#""curve":{"kind":"arc","radius":1000},"#,
                &rows(3, 50),
            )],
            47.5,
        ),
        (
            "negative arc",
            vec![block(
                r#""curve":{"kind":"arc","radius":-2500},"#,
                &rows(6, 30),
            )],
            47.5,
        ),
        (
            "bulge, stagger, skew",
            vec![block(
                r#""curve":{"kind":"bulge","amount":-80},"stagger":0.5,"skew":15,"rowPitch":120,"#,
                &rows(7, 25),
            )],
            60.0,
        ),
        ("removed", vec![block("", &removed)], 47.5),
        (
            "rotated",
            vec![block(
                r#""rotation":33.3,"origin":[400,-90],"#,
                &rows(5, 15),
            )],
            47.5,
        ),
        (
            "two blocks across an aisle",
            vec![
                block(r#""origin":[-700,0],"#, &rows(6, 20)),
                block(r#""origin":[700,0],"#, &rows(6, 20)),
            ],
            47.5,
        ),
        (
            "front and back blocks",
            vec![
                block("", &rows(4, 10)),
                block(
                    r#""origin":[0,600],"curve":{"kind":"arc","radius":3000},"#,
                    &rows(5, 40),
                ),
            ],
            47.5,
        ),
        ("one seat", vec![block("", &rows(1, 1))], 47.5),
    ];
    for (name, blocks, p) in cases {
        let s = seats(&blocks);
        let poly = auto_outline(&s, p).unwrap();
        assert!(poly.len() >= 3 && simple(&poly), "{name}: not simple");
        for &seat in &s {
            assert_eq!(winding(&poly, seat).abs(), 1, "{name}: seat outside");
            let d = boundary_distance(&poly, seat);
            assert!(d >= p - 1e-9, "{name}: padding {d} < {p}");
        }
        let (a, per) = hull_area_perimeter(&s);
        let grow = 5.0 * p;
        let bound = a + per * grow + core::f64::consts::PI * grow * grow;
        assert!(area(&poly).abs() <= bound, "{name}: too large");
    }
    // Blocks too far apart to close into one region: the convex hull.
    let s = seats(&[
        block("", &rows(2, 5)),
        block(r#""origin":[5000,3000],"#, &rows(2, 5)),
    ]);
    let poly = auto_outline(&s, 47.5).unwrap();
    assert!(simple(&poly) && s.iter().all(|&p| winding(&poly, p) != 0));
    assert_eq!(convex_hull(poly.clone()).len(), poly.len());
    // A block curved through about 160° is far from convex, and so is its
    // outline.
    let s = seats(&[block(
        r#""curve":{"kind":"arc","radius":1000},"#,
        &rows(3, 50),
    )]);
    let poly = auto_outline(&s, 47.5).unwrap();
    let (hull, _) = hull_area_perimeter(&poly);
    assert!(area(&poly).abs() < 0.6 * hull, "{} vs {hull}", area(&poly));
    assert_eq!(auto_outline(&[], 47.5), Some(Vec::new()));
}

#[test]
fn explicit_outlines_are_validated() {
    let square = [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]];
    let mut closed = square.to_vec();
    closed.extend([[0.0, 100.0], [0.0, 0.0]]);
    assert_eq!(explicit_outline(&closed), Ok(square.to_vec()));
    let bow_tie = [[0.0, 0.0], [100.0, 100.0], [100.0, 0.0], [0.0, 100.0]];
    assert_eq!(
        explicit_outline(&bow_tie),
        Err(OutlineError::SelfIntersecting)
    );
    // Touching at a vertex, and folding back along an edge.
    let touch = [[0.0, 0.0], [200.0, 0.0], [100.0, 0.0], [100.0, 100.0]];
    assert_eq!(
        explicit_outline(&touch),
        Err(OutlineError::SelfIntersecting)
    );
    let pinch = [
        [0.0, 0.0],
        [100.0, 0.0],
        [50.0, 50.0],
        [100.0, 100.0],
        [0.0, 100.0],
        [50.0, 50.0],
    ];
    assert_eq!(
        explicit_outline(&pinch),
        Err(OutlineError::SelfIntersecting)
    );
    let flat = [[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]];
    assert_eq!(explicit_outline(&flat), Err(OutlineError::ZeroSize));
    assert_eq!(
        explicit_outline(&[[0.0, f64::NAN]; 3]),
        Err(OutlineError::NotFinite)
    );
}

/// A box of `w` × `h` centred on `c` inside an axis-aligned polygon: corners
/// inside and no polygon vertex strictly inside the box.
fn box_fits(poly: &[P], c: P, w: f64, h: f64) -> bool {
    let (x0, y0, x1, y1) = (
        c[0] - w / 2.0,
        c[1] - h / 2.0,
        c[0] + w / 2.0,
        c[1] + h / 2.0,
    );
    let inside = |p: P| winding(poly, p) != 0 && boundary_distance(poly, p) > 0.0;
    [[x0, y0], [x1, y0], [x0, y1], [x1, y1]]
        .into_iter()
        .all(inside)
        && !poly
            .iter()
            .any(|v| v[0] > x0 && v[0] < x1 && v[1] > y0 && v[1] < y1)
}

/// The label size fits under the text model (0.6 em advance, 1.2 em line) and
/// a quarter unit more does not.
fn assert_largest(poly: &[P], anchor: P, chars: usize) {
    let q = label_size(poly, anchor, chars);
    let dims = |q: u16| {
        let s = f64::from(q) / 4.0;
        (0.6 * chars as f64 * s, 1.2 * s)
    };
    let (w, h) = dims(q);
    assert!(box_fits(poly, anchor, w, h), "size {q} does not fit");
    let (w, h) = dims(q + 1);
    assert!(!box_fits(poly, anchor, w, h), "size {} also fits", q + 1);
}

#[test]
fn anchors_and_label_sizes() {
    // A square: the pole is its centre.
    let square = [[0.0, 0.0], [1000.0, 0.0], [1000.0, 1000.0], [0.0, 1000.0]];
    let c = pole(&square, [1.0, 1.0]);
    assert!(
        (c[0] - 500.0).abs() <= 2.0 && (c[1] - 500.0).abs() <= 2.0,
        "{c:?}"
    );
    // "Orchestra": 9 × 0.6 × s < 1000 gives s < 185.19, so 740 quarter units.
    assert_eq!(label_size(&square, [500.0, 500.0], 9), 740);
    assert_largest(&square, [500.0, 500.0], 9);
    // An L with a 200-wide upright and a 100-tall foot: the deepest points are
    // 100 from the edge, on the upright's centre line.
    let l = [
        [0.0, 0.0],
        [200.0, 0.0],
        [200.0, 300.0],
        [400.0, 300.0],
        [400.0, 400.0],
        [0.0, 400.0],
    ];
    let c = pole(&l, [1.0, 1.0]);
    assert_eq!(winding(&l, c).abs(), 1);
    assert!(boundary_distance(&l, c) >= 100.0 - 2.0, "{c:?}");
    assert!((c[0] - 100.0).abs() <= 2.0, "{c:?}");
    assert_largest(&l, c, 4);
    assert_largest(&l, [50.0, 350.0], 1);
    // No label, or an anchor outside, has no size.
    assert_eq!(label_size(&square, [500.0, 500.0], 0), 0);
    assert_eq!(label_size(&l, [300.0, 100.0], 3), 0);
}
