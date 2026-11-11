//! Decoration shapes as straight-edged polygons (`docs/03-chart-format.md`
//! §2.7, §3.1.1): curves flattened, path data parsed, strokes built from
//! pieces. Only `+ − × ÷ √`, exact `ceil` and [`crate::detmath`], so every
//! target gets the same points; [`crate::tessellate`] fills the result.

use crate::detmath;
use crate::geometry::{MAX_POINTS, P};

/// Largest distance between a flattened curve and the true curve, in chart
/// units.
pub(crate) const TOLERANCE: f64 = 0.5;
/// Segments per quarter turn or per Bézier curve, whatever the tolerance asks.
const MAX_SEGMENTS: f64 = 256.0;
/// A miter longer than this many stroke widths becomes a bevel (SVG's
/// default limit).
const MITER_LIMIT: f64 = 4.0;

/// One subpath: its points and whether it is closed.
pub(crate) type Subpath = (Vec<P>, bool);

/// The least whole number of steps at or above `x`, at least 1; `None`
/// past [`MAX_SEGMENTS`] (or for NaN), when the curve is too large to keep
/// within [`TOLERANCE`].
fn steps(x: f64) -> Option<usize> {
    let n = x.ceil().max(1.0);
    (n <= MAX_SEGMENTS).then_some(n as usize)
}

/// `(cos θ, sin θ)` for θ at `m` equal steps over a quarter turn, both ends
/// included and exact. With radius `r` and step θ the chords are within
/// rθ²/8 of the arc, so `m` is the least that keeps `r` within
/// [`TOLERANCE`]; `None` if that is more than 256.
fn quarter(r: f64) -> Option<Vec<P>> {
    let m = steps(core::f64::consts::FRAC_PI_4 * (r / (2.0 * TOLERANCE)).sqrt())?;
    let q = (0..=m)
        .map(|k| match k {
            0 => [1.0, 0.0],
            k if k == m => [0.0, 1.0],
            k => {
                let (s, c) = detmath::sin_cos(core::f64::consts::FRAC_PI_2 * k as f64 / m as f64);
                [c, s]
            }
        })
        .collect();
    Some(q)
}

/// A `w × h` rectangle centred on the origin, its corners rounded by
/// `radius` (at most half the shorter side), clockwise on screen.
pub(crate) fn rect(w: f64, h: f64, radius: f64) -> Option<Vec<P>> {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let r = radius.min(hw).min(hh);
    if r <= 0.0 {
        return Some(vec![[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]]);
    }
    let q = quarter(r)?;
    // Corner centres, and the quarter's axes, starting at the top right.
    let corners = [
        ([hw - r, -hh + r], [0.0, -1.0], [1.0, 0.0]),
        ([hw - r, hh - r], [1.0, 0.0], [0.0, 1.0]),
        ([-hw + r, hh - r], [0.0, 1.0], [-1.0, 0.0]),
        ([-hw + r, -hh + r], [-1.0, 0.0], [0.0, -1.0]),
    ];
    let mut out: Vec<P> = Vec::with_capacity(4 * q.len());
    for (c, u, v) in corners {
        for &[a, b] in &q {
            let p = [
                c[0] + r * (a * u[0] + b * v[0]),
                c[1] + r * (a * u[1] + b * v[1]),
            ];
            if out.last() != Some(&p) {
                out.push(p);
            }
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    Some(out)
}

/// An ellipse centred on the origin with radii `a` and `b`, at equal steps
/// of the parameter angle: the affine image of a circle flattened for the
/// larger radius, so within [`TOLERANCE`] of the true curve.
pub(crate) fn ellipse(a: f64, b: f64) -> Option<Vec<P>> {
    let q = quarter(a.max(b))?;
    let m = q.len().saturating_sub(1);
    let mut out = Vec::with_capacity(4 * m);
    // Quarters clockwise from +x; odd ones run backwards through the table:
    // cos(90° + φ) = −sin φ = −cos(90° − φ), and so on.
    for (sx, sy, backwards) in [
        (1.0, 1.0, false),
        (-1.0, 1.0, true),
        (-1.0, -1.0, false),
        (1.0, -1.0, true),
    ] {
        for k in 0..m {
            let [c, s] = q
                .get(if backwards { m - k } else { k })
                .copied()
                .unwrap_or_default();
            out.push([sx * a * c, sy * b * s]);
        }
    }
    Some(out)
}

/// Points of a quadratic or cubic Bézier curve after its first, at `n`
/// equal parameter steps. Each chord is within max|B''| / (8n²) of the
/// curve, and |B''| is at most 2|p0 − 2p1 + p2| (quadratic) or
/// 6 max(|p0 − 2p1 + p2|, |p1 − 2p2 + p3|) (cubic).
/// `None`, adding nothing, when that takes more than 256 steps.
fn bezier(ctrl: &[P], out: &mut Vec<P>) -> Option<()> {
    let second = |a: P, b: P, c: P| {
        let v = [a[0] - 2.0 * b[0] + c[0], a[1] - 2.0 * b[1] + c[1]];
        (v[0] * v[0] + v[1] * v[1]).sqrt()
    };
    let bound = match *ctrl {
        [p0, p1, p2] => 2.0 * second(p0, p1, p2),
        [p0, p1, p2, p3] => 6.0 * second(p0, p1, p2).max(second(p1, p2, p3)),
        _ => return Some(()),
    };
    let n = steps((bound / (8.0 * TOLERANCE)).sqrt())?;
    for k in 1..=n {
        let t = k as f64 / n as f64;
        let u = 1.0 - t;
        let w: [f64; 4] = match ctrl.len() {
            3 => [u * u, 2.0 * u * t, t * t, 0.0],
            _ => [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t],
        };
        let mut p = [0.0; 2];
        for (c, w) in ctrl.iter().zip(w) {
            p = [p[0] + w * c[0], p[1] + w * c[1]];
        }
        out.push(p);
    }
    Some(())
}

/// Why path data was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathError {
    /// Not valid path data.
    Syntax,
    /// An elliptical arc command (`A`, `a`).
    Arc,
    /// A curve that needs more than 256 steps to stay within [`TOLERANCE`].
    CurveTooLarge,
    /// More than [`MAX_POINTS`] points once flattened.
    TooManyPoints,
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn skip(&mut self) {
        while let [b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b',', rest @ ..] = self.0 {
            self.0 = rest;
        }
    }

    /// The next command letter, if the next token is one.
    fn command(&mut self) -> Option<u8> {
        self.skip();
        let (&c, rest) = self.0.split_first()?;
        c.is_ascii_alphabetic().then(|| {
            self.0 = rest;
            c
        })
    }

    fn at_number(&mut self) -> bool {
        self.skip();
        matches!(self.0.first(), Some(b'0'..=b'9' | b'+' | b'-' | b'.'))
    }

    fn number(&mut self) -> Result<f64, PathError> {
        self.skip();
        let s = self.0;
        let mut i = 0;
        let digits = |i: &mut usize| {
            let from = *i;
            while s.get(*i).is_some_and(u8::is_ascii_digit) {
                *i += 1;
            }
            *i > from
        };
        if let Some(b'+' | b'-') = s.first() {
            i += 1;
        }
        let mut any = digits(&mut i);
        if s.get(i) == Some(&b'.') {
            i += 1;
            any |= digits(&mut i);
        }
        if !any {
            return Err(PathError::Syntax);
        }
        if let Some(b'e' | b'E') = s.get(i) {
            i += 1;
            if let Some(b'+' | b'-') = s.get(i) {
                i += 1;
            }
            if !digits(&mut i) {
                return Err(PathError::Syntax);
            }
        }
        let (text, rest) = s.split_at_checked(i).ok_or(PathError::Syntax)?;
        self.0 = rest;
        // Rust's float parsing is correctly rounded, so exact everywhere.
        let v: f64 = core::str::from_utf8(text)
            .ok()
            .and_then(|t| t.parse().ok())
            .ok_or(PathError::Syntax)?;
        v.is_finite().then_some(v).ok_or(PathError::Syntax)
    }

    fn point(&mut self, rel: bool, cur: P) -> Result<P, PathError> {
        let (x, y) = (self.number()?, self.number()?);
        Ok(if rel {
            [cur[0] + x, cur[1] + y]
        } else {
            [x, y]
        })
    }
}

/// Parses SVG path data into flattened subpaths: `M L H V C S Q T Z` in
/// absolute and relative form, with implicit repeats. Elliptical arcs are
/// refused. A subpath of one point is dropped.
pub(crate) fn parse_path(d: &str) -> Result<Vec<Subpath>, PathError> {
    let mut c = Cursor(d.as_bytes());
    let mut paths: Vec<Subpath> = Vec::new();
    let mut pts: Vec<P> = Vec::new();
    let (mut cur, mut start) = ([0.0; 2], [0.0; 2]);
    // The last command's second control point, for `S` and `T`.
    let mut last_ctrl: Option<(u8, P)> = None;
    let mut done = 0usize;
    let finish = |pts: &mut Vec<P>, closed: bool, paths: &mut Vec<Subpath>, done: &mut usize| {
        if pts.len() >= 2 {
            *done += pts.len();
            paths.push((std::mem::take(pts), closed));
        }
        pts.clear();
    };
    // A leading `m` is relative to (0, 0); its implicit repeats stay relative.
    let mut cmd = match c.command() {
        Some(m @ (b'M' | b'm')) => m,
        _ => return Err(PathError::Syntax),
    };
    loop {
        let rel = cmd.is_ascii_lowercase();
        let reflect = |kind: u8| match last_ctrl {
            Some((k, p)) if k == kind => [2.0 * cur[0] - p[0], 2.0 * cur[1] - p[1]],
            _ => cur,
        };
        let mut ctrl = None;
        match cmd.to_ascii_uppercase() {
            b'M' => {
                finish(&mut pts, false, &mut paths, &mut done);
                cur = c.point(rel, cur)?;
                start = cur;
                pts.push(cur);
                // Further pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                cur = c.point(rel, cur)?;
                pts.push(cur);
            }
            b'H' => {
                let x = c.number()?;
                cur[0] = if rel { cur[0] + x } else { x };
                pts.push(cur);
            }
            b'V' => {
                let y = c.number()?;
                cur[1] = if rel { cur[1] + y } else { y };
                pts.push(cur);
            }
            b'C' | b'S' => {
                let p1 = if cmd.eq_ignore_ascii_case(&b'S') {
                    reflect(b'C')
                } else {
                    c.point(rel, cur)?
                };
                let p2 = c.point(rel, cur)?;
                let p3 = c.point(rel, cur)?;
                bezier(&[cur, p1, p2, p3], &mut pts).ok_or(PathError::CurveTooLarge)?;
                ctrl = Some((b'C', p2));
                cur = p3;
            }
            b'Q' | b'T' => {
                let p1 = if cmd.eq_ignore_ascii_case(&b'T') {
                    reflect(b'Q')
                } else {
                    c.point(rel, cur)?
                };
                let p2 = c.point(rel, cur)?;
                bezier(&[cur, p1, p2], &mut pts).ok_or(PathError::CurveTooLarge)?;
                ctrl = Some((b'Q', p1));
                cur = p2;
            }
            b'Z' => {
                finish(&mut pts, true, &mut paths, &mut done);
                cur = start;
                pts.push(cur);
            }
            b'A' => return Err(PathError::Arc),
            _ => return Err(PathError::Syntax),
        }
        last_ctrl = ctrl;
        if done + pts.len() > MAX_POINTS {
            return Err(PathError::TooManyPoints);
        }
        if cmd.eq_ignore_ascii_case(&b'Z') || !c.at_number() {
            match c.command() {
                Some(next) => cmd = next,
                None if c.0.is_empty() => break,
                None => return Err(PathError::Syntax),
            }
        }
    }
    finish(&mut pts, false, &mut paths, &mut done);
    Ok(paths)
}

/// Appends convex polygons that together make the stroke of `pts` at half
/// width `h`, with butt ends. Consecutive points must differ.
///
/// Each segment is a quadrilateral across the stroke. At a join with turn φ
/// the two segments meet on the inner side at the inner miter point, which
/// is h·tan(φ/2) back along each, and the outer corner is filled with a
/// miter or, past [`MITER_LIMIT`] or when the miter adds no more than
/// [`TOLERANCE`], a bevel; the pieces then tile the stroke. Where a segment
/// is shorter than twice that distance, the join keeps butt ends instead and
/// the two segments overlap on its inner side.
pub(crate) fn stroke(pts: &[P], closed: bool, h: f64, out: &mut Vec<Vec<P>>) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let segments = if closed { n } else { n - 1 };
    let at = |i: usize| pts.get(i % n).copied().unwrap_or_default();
    // Unit direction and length of segment i.
    let dir = |i: usize| {
        let (a, b) = (at(i), at(i + 1));
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        ([d[0] / len, d[1] / len], len)
    };
    let off = |p: P, v: P, s: f64| [p[0] + s * v[0], p[1] + s * v[1]];
    // Per point: where the segment before it ends and the one after it
    // starts, each as (left, right) corners, left being the +normal side
    // with normal (−u.y, u.x); and the outer piece, if any.
    let mut ends: Vec<(P, P)> = Vec::with_capacity(n);
    let mut starts: Vec<(P, P)> = Vec::with_capacity(n);
    for i in 0..n {
        let p = at(i);
        let (u2, len2) = dir(i);
        let n2 = [-u2[1], u2[0]];
        let butt2 = (off(p, n2, h), off(p, n2, -h));
        let is_join = closed || (i > 0 && i + 1 < n);
        if !is_join {
            starts.push(butt2);
            let (u1, _) = dir(i + n - 1);
            let n1 = [-u1[1], u1[0]];
            ends.push((off(p, n1, h), off(p, n1, -h)));
            continue;
        }
        let (u1, len1) = dir(i + n - 1);
        let n1 = [-u1[1], u1[0]];
        let butt1 = (off(p, n1, h), off(p, n1, -h));
        let dot = u1[0] * u2[0] + u1[1] * u2[1];
        // n1 · u2: positive when the path turns towards n1, the left.
        let turn = n1[0] * u2[0] + n1[1] * u2[1];
        if turn == 0.0 {
            // Straight on: the butt ends coincide. Straight back: no join.
            ends.push(butt1);
            starts.push(butt2);
            continue;
        }
        // The outer side, and the offsets that reach the inner and outer
        // miter points: (n1 + n2) h / (1 + cos φ), at h / cos(φ/2) from p.
        let s = if turn > 0.0 { -h } else { h };
        let m = [(n1[0] + n2[0]) / (1.0 + dot), (n1[1] + n2[1]) / (1.0 + dot)];
        let (a, b) = (off(p, n1, s), off(p, n2, s));
        // h·tan(φ/2) = h·|sin φ| / (1 + cos φ).
        let back = h * turn.abs() / (1.0 + dot);
        let inner = if back <= len1 / 2.0 && back <= len2 / 2.0 {
            let q = off(p, m, -s);
            ends.push(if s > 0.0 { (a, q) } else { (q, a) });
            starts.push(if s > 0.0 { (b, q) } else { (q, b) });
            q
        } else {
            ends.push(butt1);
            starts.push(butt2);
            p
        };
        // The miter tip lies h·sin²(φ/2)/cos(φ/2) = h (1 − cos φ) /
        // √(2 (1 + cos φ)) beyond the bevel. Within the limit when
        // 1 + cos φ ≥ 2 / limit².
        let tip = h * (1.0 - dot) / (2.0 * (1.0 + dot)).sqrt();
        if 1.0 + dot >= 2.0 / (MITER_LIMIT * MITER_LIMIT) && tip > TOLERANCE {
            out.push(vec![inner, a, off(p, m, s), b]);
        } else {
            out.push(vec![inner, a, b]);
        }
    }
    for i in 0..segments {
        let (Some(&(l0, r0)), Some(&(l1, r1))) = (starts.get(i), ends.get((i + 1) % n)) else {
            continue;
        };
        out.push(vec![l0, l1, r1, r0]);
    }
}

#[cfg(test)]
mod tests;
