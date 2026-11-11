//! Snapping (`docs/07-designer.md` §5.2): a proposed position, move, angle or
//! direction is pulled onto the nearest target within `tuning::SNAP` CSS px,
//! each axis on its own. Targets of a better kind win over nearer ones of a
//! worse kind; among one kind the nearest, unless the target held at the last
//! event is still in reach (so a jittering pointer does not flicker between
//! two targets). Alt turns snapping off for the event. What snapped is
//! described as [`Guide`]s for the canvas.

use cavea_compiler::document::{Curve, Node};

use super::{Context, Pointer, tuning};
use crate::draft::normalise;
use crate::index::{P, Place, Rect};
use crate::selection::shown;

/// Which targets snap, and the grid step in chart units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapSettings {
    pub grid: bool,
    pub step: f64,
    /// Edges and centres of other nodes, and row ends.
    pub objects: bool,
    /// Equal spacing between neighbours.
    pub spacing: bool,
    /// A dragged block's rows on a neighbour's row lines.
    pub rows: bool,
    /// 15° steps, and a block facing the floor's focal point.
    pub angles: bool,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            grid: true,
            step: 5.0,
            objects: true,
            spacing: true,
            rows: true,
            angles: true,
        }
    }
}

/// What the canvas draws for a snap, in chart coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Guide {
    /// An edge, centre or row end lined up: a magenta line.
    Line { from: P, to: P },
    /// One of two equal gaps, with its length.
    Spacing { from: P, to: P, distance: f64 },
    /// A neighbour's row line, extended through the dragged block: dashed.
    Row { from: P, to: P },
    /// An angle about `centre`, from and to these directions (degrees,
    /// clockwise from +x).
    Angle { centre: P, from: f64, to: f64 },
}

/// Target kinds, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Rows,
    Object,
    Spacing,
    Grid,
}

#[derive(Debug, Clone)]
struct Cand {
    kind: Kind,
    /// The correction along the axis.
    off: f64,
    /// What is snapped to: the identity kept between events.
    target: f64,
    /// For an object: the box lined up with, for its guide line.
    with: Option<Rect>,
    guides: Vec<Guide>,
}

/// The session's snapping state: the settings, the targets held at the last
/// event (x, y, angle), and the guides of the last event.
#[derive(Debug, Default)]
pub(crate) struct Snap {
    pub(crate) settings: SnapSettings,
    held: [Option<(Kind, f64)>; 3],
    pub(crate) guides: Vec<Guide>,
}

/// The best candidate: the best kind; within it the one held, else the
/// nearest (then the smaller target). Remembers it in `held`.
fn choose<'a>(cands: &'a [Cand], held: &mut Option<(Kind, f64)>) -> Option<&'a Cand> {
    let best = cands.iter().map(|c| c.kind).min();
    let of_best = cands.iter().filter(|c| Some(c.kind) == best);
    let kept = of_best.clone().find(|c| *held == Some((c.kind, c.target)));
    let pick = kept.or_else(|| {
        of_best.min_by(|a, b| {
            (a.off.abs().total_cmp(&b.off.abs())).then(a.target.total_cmp(&b.target))
        })
    });
    *held = pick.map(|c| (c.kind, c.target));
    pick
}

fn mid(r: &Rect, a: usize) -> f64 {
    (r.min[a] + r.max[a]) / 2.0
}

/// A box's min, centre and max along axis `a`.
fn stops(r: &Rect, a: usize) -> [f64; 3] {
    [r.min[a], mid(r, a), r.max[a]]
}

/// The point with `x` on axis `a` and `y` on the other.
fn pt(a: usize, x: f64, y: f64) -> P {
    if a == 0 { [x, y] } else { [y, x] }
}

fn shift(r: &Rect, v: P) -> Rect {
    Rect::new(
        [r.min[0] + v[0], r.min[1] + v[1]],
        [r.max[0] + v[0], r.max[1] + v[1]],
    )
}

impl Snap {
    /// Whether this event snaps at all, and the threshold in chart units.
    fn on(&self, p: &Pointer) -> Option<f64> {
        let s = &self.settings;
        let any = s.grid || s.objects || s.spacing || s.rows || s.angles;
        let thr = tuning::SNAP / p.zoom;
        (any && !p.mods.alt && thr.is_finite()).then_some(thr)
    }

    fn grid(&self, v: f64, thr: f64, out: &mut Vec<Cand>) {
        let step = self.settings.step;
        if self.settings.grid && step.is_finite() && step > 0.0 {
            let target = (v / step).round() * step;
            if (target - v).abs() <= thr && target.is_finite() {
                out.push(cand(Kind::Grid, target - v, target));
            }
        }
    }

    /// Keeps the chosen candidates of both axes: the corrected vector and
    /// their guides, an object's line spanning its box, the moved box after
    /// the correction and `at`.
    fn finish(&mut self, cands: [Vec<Cand>; 2], moved: &Rect, at: P) -> P {
        let [x, y] = &cands;
        let picks = [choose(x, &mut self.held[0]), choose(y, &mut self.held[1])];
        let off = picks.map(|c| c.map_or(0.0, |c| c.off));
        let (after, at) = (shift(moved, off), [at[0] + off[0], at[1] + off[1]]);
        for (a, c) in picks.iter().enumerate() {
            let Some(c) = c else {
                continue;
            };
            self.guides.extend(c.guides.iter().cloned());
            if let Some(w) = c.with {
                let o = 1 - a;
                let from = w.min[o].min(after.min[o]).min(at[o]);
                let to = w.max[o].max(after.max[o]).max(at[o]);
                self.guides.push(Guide::Line {
                    from: pt(a, c.target, from),
                    to: pt(a, c.target, to),
                });
            }
        }
        off
    }
}

fn cand(kind: Kind, off: f64, target: f64) -> Cand {
    Cand {
        kind,
        off,
        target,
        with: None,
        guides: Vec::new(),
    }
}

/// The world point to use for a pointer at `p` while drawing: snapped to
/// the grid and to edges, centres and row ends of nodes within
/// `tuning::SNAP_REACH` px, except `exclude` (the node being drawn).
pub(crate) fn point(cx: &mut Context<'_>, p: &Pointer, exclude: Option<u32>) -> P {
    point_with(cx, p, exclude, true)
}

/// A line's end: onto edges, centres and row ends, else onto a 15° step
/// from `from`, else onto the grid (which, finer than the snap distance,
/// would otherwise always win).
pub(crate) fn line_end(cx: &mut Context<'_>, p: &Pointer, from: P, exclude: Option<u32>) -> P {
    let mut at = point_with(cx, p, exclude, false);
    if at == p.world {
        at = direction(cx, p, from, at);
    }
    if at == p.world {
        at = point_with(cx, p, exclude, true);
    }
    at
}

fn point_with(cx: &mut Context<'_>, p: &Pointer, exclude: Option<u32>, grid: bool) -> P {
    let Some(thr) = cx.snap.on(p) else {
        return p.world;
    };
    let at = p.world;
    let mut cands: [Vec<Cand>; 2] = Default::default();
    if cx.snap.settings.objects {
        let reach = Rect::new(at, at).grow(tuning::SNAP_REACH / p.zoom);
        let exclude: Vec<u32> = exclude.into_iter().collect();
        let doc = cx.editor.document();
        let anchors = cx.index.anchors(reach, &exclude);
        for anchor in anchors.iter().filter(|a| shown(doc, cx.floor, a.node)) {
            for (a, list) in cands.iter_mut().enumerate() {
                let off = anchor.point[a] - at[a];
                if off.abs() <= thr {
                    let at = Rect::new(anchor.point, anchor.point);
                    list.push(Cand {
                        with: Some(at),
                        ..cand(Kind::Object, off, anchor.point[a])
                    });
                }
            }
        }
    }
    for (a, list) in cands.iter_mut().enumerate().filter(|_| grid) {
        cx.snap.grid(at[a], thr, list);
    }
    let point = Rect::new(at, at);
    let off = cx.snap.finish(cands, &point, at);
    [at[0] + off[0], at[1] + off[1]]
}

/// A selection being moved: its world bounds at the start, its parent, the
/// nodes snapping ignores (the moved ones, what they hold and what holds
/// them; sorted), and a single straight row block's origin and rotation
/// in the parent's space at the start.
#[derive(Debug)]
pub(crate) struct Moving {
    start: Rect,
    parent: Option<u32>,
    exclude: Vec<u32>,
    block: Option<(P, f64)>,
}

impl Moving {
    pub(crate) fn new(cx: &Context<'_>, ids: &[u32], parent: Option<u32>) -> Option<Moving> {
        let doc = cx.editor.document();
        let start = ids
            .iter()
            .fold(None, |r, &id| Rect::union(r, cx.index.bounds(id)))?;
        let mut exclude: Vec<u32> = ids.to_vec();
        for &id in ids {
            exclude.extend(doc.children(Some(id)));
        }
        exclude.extend(parent);
        exclude.sort_unstable();
        exclude.dedup();
        let block = match ids {
            [id] => match doc.node(*id) {
                Some(Node::RowBlock(b)) if matches!(b.curve, Curve::None {}) => {
                    Some((b.origin, b.rotation))
                }
                _ => None,
            },
            _ => None,
        };
        Some(Moving {
            start,
            parent,
            exclude,
            block,
        })
    }
}

/// The world move to use for a drag by `delta` (world): the moved box's
/// edges and centre onto other nodes' edges and centres, an equal gap to a
/// neighbour, a single block's rows onto a neighbour's row lines, and the
/// box's top left onto the grid.
pub(crate) fn translation(cx: &mut Context<'_>, p: &Pointer, m: &Moving, delta: P) -> P {
    let Some(thr) = cx.snap.on(p) else {
        return delta;
    };
    let s = cx.snap.settings;
    let moved = shift(&m.start, delta);
    let reach = moved.grow(tuning::SNAP_REACH / p.zoom);
    let doc = cx.editor.document();
    let near: Vec<(u32, Rect)> = cx
        .index
        .near(reach)
        .filter(|(id, _)| m.exclude.binary_search(id).is_err() && shown(doc, cx.floor, *id))
        .collect();
    let siblings: Vec<Rect> = near
        .iter()
        .filter(|(id, _)| doc.parent(*id) == m.parent)
        .map(|(_, r)| *r)
        .collect();
    let mut cands: [Vec<Cand>; 2] = Default::default();
    for (a, list) in cands.iter_mut().enumerate() {
        if s.objects {
            for (_, b) in &near {
                for v in stops(&moved, a) {
                    for t in stops(b, a) {
                        if (t - v).abs() <= thr {
                            list.push(Cand {
                                with: Some(*b),
                                ..cand(Kind::Object, t - v, t)
                            });
                        }
                    }
                }
            }
        }
        if s.spacing {
            spacing(a, &moved, &siblings, thr, list);
        }
        cx.snap.grid(moved.min[a], thr, list);
    }
    if s.rows
        && let Some(rows) = rows(cx, m, delta, &near, thr)
    {
        match rows.axis {
            Some(a) => {
                if let Some(list) = cands.get_mut(a) {
                    list.push(rows.cand);
                }
            }
            // A turned block: the rows alone decide.
            None => {
                cx.snap.held = [None; 3];
                cx.snap.guides.extend(rows.cand.guides);
                return [delta[0] + rows.v[0], delta[1] + rows.v[1]];
            }
        }
    }
    let off = cx.snap.finish(cands, &moved, mid2(&moved));
    [delta[0] + off[0], delta[1] + off[1]]
}

fn mid2(r: &Rect) -> P {
    [mid(r, 0), mid(r, 1)]
}

/// Equal spacing along axis `a`: the moved box placed so that its gap to the
/// nearest sibling on one side (overlapping it across the axis) equals the
/// gap between that sibling and the next one beyond it.
fn spacing(a: usize, moved: &Rect, siblings: &[Rect], thr: f64, out: &mut Vec<Cand>) {
    let o = 1 - a;
    let across = |r: &Rect, s: &Rect| r.min[o] < s.max[o] && s.min[o] < r.max[o];
    // Towards −axis (`s` −1) a box's `near` edge, facing the moved box, is
    // its max and its `far` edge its min; towards +axis the reverse.
    for s in [-1.0, 1.0] {
        let near = |r: &Rect| if s < 0.0 { r.max[a] } else { r.min[a] };
        let far = |r: &Rect| if s < 0.0 { r.min[a] } else { r.max[a] };
        // The first sibling across `of` whose near edge is beyond `from`.
        let next = |from: f64, of: &Rect| {
            siblings
                .iter()
                .filter(|x| across(x, of) && s * (near(x) - from) >= 0.0)
                .min_by(|x, y| (s * near(x)).total_cmp(&(s * near(y))))
        };
        let Some(n1) = next(far(moved) - s * thr, moved) else {
            continue;
        };
        let Some(n2) = next(far(n1), n1) else {
            continue;
        };
        let gap = s * (near(n2) - far(n1));
        let target = near(n1) - s * gap;
        let off = target - far(moved);
        if off.abs() > thr || !target.is_finite() {
            continue;
        }
        let y = mid(n1, o);
        let mark = |x0: f64, x1: f64| Guide::Spacing {
            from: pt(a, x0.min(x1), y),
            to: pt(a, x0.max(x1), y),
            distance: gap,
        };
        out.push(Cand {
            guides: vec![mark(far(n1), near(n2)), mark(target, near(n1))],
            ..cand(Kind::Spacing, off, target)
        });
    }
}

struct RowSnap {
    cand: Cand,
    /// The world axis the correction lies on, if it is axis-aligned.
    axis: Option<usize>,
    /// The correction as a world vector.
    v: P,
}

/// A single straight block's front row onto the nearest row line of a
/// sibling straight block turned the same way: along that block's own y,
/// to a whole number of its row pitches from its front row.
fn rows(cx: &Context<'_>, m: &Moving, delta: P, near: &[(u32, Rect)], thr: f64) -> Option<RowSnap> {
    let (o0, r) = m.block?;
    let doc = cx.editor.document();
    let parent = m.parent.and_then(|s| cx.index.place(s));
    let v = cx.index.to_parent(m.parent, delta);
    let o = [o0[0] + v[0], o0[1] + v[1]];
    let mut best: Option<(f64, u32, P, P)> = None;
    for &(id, _) in near {
        let Some(Node::RowBlock(b)) = doc.node(id) else {
            continue;
        };
        let same = doc.parent(id) == m.parent && normalise(b.rotation - r) == 0.0;
        if !same
            || !matches!(b.curve, Curve::None {})
            || b.row_pitch.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
        {
            continue;
        }
        let n = Place::of([0.0; 2], b.rotation).turn([0.0, 1.0]);
        let y = (o[0] - b.origin[0]) * n[0] + (o[1] - b.origin[1]) * n[1];
        let line = (y / b.row_pitch).round() * b.row_pitch;
        let off = line - y;
        if off.abs() <= thr && best.is_none_or(|x| off.abs() < x.0.abs()) {
            let base = [b.origin[0] + line * n[0], b.origin[1] + line * n[1]];
            best = Some((off, id, n, base));
        }
    }
    let (off, id, n, base) = best?;
    let turn = |v: P| parent.map_or(v, |pl| pl.turn(v));
    let v = turn([off * n[0], off * n[1]]);
    let nw = turn(n);
    // The guide: the row line, across both blocks.
    let u = turn(Place::of([0.0; 2], r).turn([1.0, 0.0]));
    let q = parent.map_or(base, |pl| pl.apply(base));
    let moved = shift(&m.start, [delta[0] + v[0], delta[1] + v[1]]);
    let corners = cx.index.quad(id)?.into_iter().chain(moved.corners());
    let ts = corners.map(|c| (c[0] - q[0]) * u[0] + (c[1] - q[1]) * u[1]);
    let (t0, t1) = ts.fold((f64::MAX, f64::MIN), |(a, b), t| (a.min(t), b.max(t)));
    let mut cand = cand(Kind::Rows, 0.0, 0.0);
    cand.guides.push(Guide::Row {
        from: [q[0] + t0 * u[0], q[1] + t0 * u[1]],
        to: [q[0] + t1 * u[0], q[1] + t1 * u[1]],
    });
    let axis = [0, 1].into_iter().find(|&a| nw[1 - a].abs() < 1e-12);
    if let Some(a) = axis {
        (cand.off, cand.target) = (v[a], mid(&moved, a));
    }
    Some(RowSnap { cand, axis, v })
}

/// The angular threshold, in degrees, at the pointer's distance from
/// `centre`; `None` when snapping is off or the pointer is on the centre.
fn angle_reach(cx: &Context<'_>, p: &Pointer, centre: P) -> Option<f64> {
    let thr = cx.snap.on(p)?;
    let r = (p.world[0] - centre[0]).hypot(p.world[1] - centre[1]);
    (cx.snap.settings.angles && r > thr).then(|| (thr / r).to_degrees())
}

/// A rotation by `a` degrees about `pivot`, started at direction `start`:
/// onto a multiple of 15°, or, for `facing` (the rotation that turns a
/// block's front towards the floor's focal point), onto that first.
pub(crate) fn angle(
    cx: &mut Context<'_>,
    p: &Pointer,
    (pivot, start): (P, f64),
    a: f64,
    facing: Option<f64>,
) -> f64 {
    let Some(reach) = angle_reach(cx, p, pivot) else {
        return a;
    };
    let near = |t: f64| (normalise(t - a).abs() <= reach).then(|| cand(Kind::Object, 0.0, t));
    let step = (a / 15.0).round() * 15.0;
    let list: Vec<Cand> = facing
        .and_then(near)
        .into_iter()
        .chain(near(step).map(|c| Cand {
            kind: Kind::Grid,
            ..c
        }))
        .collect();
    let Some(t) = choose(&list, &mut cx.snap.held[2]).map(|c| c.target) else {
        return a;
    };
    cx.snap.guides.push(Guide::Angle {
        centre: pivot,
        from: start,
        to: start + t,
    });
    t
}

/// The direction from `from` to `to` (world), onto a multiple of 15°,
/// keeping its length.
fn direction(cx: &mut Context<'_>, p: &Pointer, from: P, to: P) -> P {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let theta = dy.atan2(dx).to_degrees();
    let t = angle(cx, p, (from, 0.0), theta, None);
    if t == theta {
        return to;
    }
    let (s, c) = t.to_radians().sin_cos();
    let r = dx.hypot(dy);
    [from[0] + r * c, from[1] + r * s]
}

/// The rotation, in degrees, that turns row block `id` (its front, −y)
/// towards the focal point of the floor shown, as seen from `pivot`.
pub(crate) fn facing(cx: &Context<'_>, ids: &[u32], pivot: P) -> Option<f64> {
    let doc = cx.editor.document();
    let [id] = ids else {
        return None;
    };
    let Node::RowBlock(b) = doc.node(*id)? else {
        return None;
    };
    let floor = doc
        .floors()
        .iter()
        .find(|f| Some(f.id) == cx.floor)
        .or(doc.floors().first())?;
    let f = floor.focal_point?;
    let (dx, dy) = (f[0] - pivot[0], f[1] - pivot[1]);
    let held = doc.parent(*id).and_then(|s| doc.node(s));
    let section = held.map_or(0.0, |s| crate::document::each_node!(s, n => n.rotation));
    let want = dx.atan2(-dy).to_degrees();
    Some(normalise(want - b.rotation - section))
}
